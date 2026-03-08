use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{IVec2, Vec3};

use super::delta_store::DeltaStore;
use crate::world_core::biome::{classify, Biome};
use crate::world_core::chunk::{ChunkData, PlantInstance, CHUNK_SIZE_METERS};
use crate::world_core::chunk_generator::ChunkGenerator;
use crate::world_core::config::GameConfig;
use crate::world_core::content::sampling::{
    estimate_slope, hash4, hash_to_unit_float, sample_field_bilinear,
};
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::lifecycle::{
    advance_delta_plant_growth, assemble_plants, ChunkDelta, DeltaPlant, GrowthStage,
    MAX_CATCH_UP_HOURS,
};

pub struct StreamingStats {
    pub loaded_chunks: usize,
    pub pending_chunks: usize,
    pub center_chunk: IVec2,
}

struct PlantLandingRules<'a> {
    registry: &'a PlantRegistry,
    biome_config: &'a crate::world_core::config::BiomeConfig,
    sea_level: f32,
}

struct LifecycleTickContext<'a> {
    loaded: &'a HashMap<IVec2, ChunkData>,
    total_hours: f64,
    world_seed: u32,
    landing_rules: PlantLandingRules<'a>,
}

struct SeedlingSpawnRequest {
    coord: IVec2,
    plant_index: u32,
    seed_i: u32,
    source_position: Vec3,
    species_index: usize,
    round_hour: f64,
}

// ---------------------------------------------------------------------------
// ChunkLoader trait — abstracts platform-specific chunk generation strategy
// ---------------------------------------------------------------------------

trait ChunkLoader {
    fn new_loader(
        seed: u32,
        threads: usize,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn dispatch(&mut self, coord: IVec2, seed: u32);
    fn poll(&mut self) -> Vec<ChunkData>;
    fn pending_count(&self) -> usize;
    fn cancel_outside(&mut self, required: &HashSet<IVec2>);
}

// ---------------------------------------------------------------------------
// Native: threaded chunk generation via rayon
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod threaded {
    use super::*;
    use std::sync::mpsc::{self, Receiver, Sender};

    use rayon::{ThreadPool, ThreadPoolBuilder};

    pub struct ThreadedLoader {
        pool: ThreadPool,
        sender: Sender<ChunkData>,
        receiver: Receiver<ChunkData>,
        pending: HashSet<IVec2>,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
    }

    impl ChunkLoader for ThreadedLoader {
        fn new_loader(
            seed: u32,
            threads: usize,
            config: Arc<GameConfig>,
            registry: Arc<PlantRegistry>,
        ) -> anyhow::Result<Self> {
            let _ = seed; // seed is passed per-dispatch, not stored
            let pool = ThreadPoolBuilder::new()
                .num_threads(threads.max(1))
                .thread_name(|i| format!("chunk-gen-{i}"))
                .build()?;
            let (sender, receiver) = mpsc::channel();
            Ok(Self {
                pool,
                sender,
                receiver,
                pending: HashSet::new(),
                config,
                registry,
            })
        }

        fn dispatch(&mut self, coord: IVec2, seed: u32) {
            if self.pending.contains(&coord) {
                return;
            }
            self.pending.insert(coord);
            let tx = self.sender.clone();
            let config = Arc::clone(&self.config);
            let registry = Arc::clone(&self.registry);
            self.pool.spawn(move || {
                let generator = ChunkGenerator::new(seed, &config, registry);
                let chunk = generator.generate_chunk(coord);
                let _ = tx.send(chunk);
            });
        }

        fn poll(&mut self) -> Vec<ChunkData> {
            let mut completed = Vec::new();
            while let Ok(chunk) = self.receiver.try_recv() {
                self.pending.remove(&chunk.coord);
                completed.push(chunk);
            }
            completed
        }

        fn pending_count(&self) -> usize {
            self.pending.len()
        }

        fn cancel_outside(&mut self, required: &HashSet<IVec2>) {
            self.pending.retain(|coord| required.contains(coord));
        }
    }
}

// ---------------------------------------------------------------------------
// Wasm: synchronous chunk generation, throttled per frame
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod sync {
    use super::*;

    pub struct SyncLoader {
        seed: u32,
        queue: Vec<IVec2>,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
    }

    impl ChunkLoader for SyncLoader {
        fn new_loader(
            seed: u32,
            _threads: usize,
            config: Arc<GameConfig>,
            registry: Arc<PlantRegistry>,
        ) -> anyhow::Result<Self> {
            Ok(Self {
                seed,
                queue: Vec::new(),
                config,
                registry,
            })
        }

        fn dispatch(&mut self, coord: IVec2, _seed: u32) {
            if !self.queue.contains(&coord) {
                self.queue.push(coord);
            }
        }

        fn poll(&mut self) -> Vec<ChunkData> {
            let generator =
                ChunkGenerator::new(self.seed, &self.config, Arc::clone(&self.registry));
            let count = self.queue.len().min(2);
            let coords: Vec<IVec2> = self.queue.drain(..count).collect();
            coords
                .into_iter()
                .map(|coord| generator.generate_chunk(coord))
                .collect()
        }

        fn pending_count(&self) -> usize {
            self.queue.len()
        }

        fn cancel_outside(&mut self, required: &HashSet<IVec2>) {
            self.queue.retain(|coord| required.contains(coord));
        }
    }
}

// ---------------------------------------------------------------------------
// StreamingWorld — unified orchestration, delegates loading to PlatformLoader
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
type PlatformLoader = threaded::ThreadedLoader;
#[cfg(target_arch = "wasm32")]
type PlatformLoader = sync::SyncLoader;

pub struct StreamingWorld {
    seed: u32,
    load_radius: i32,
    loaded: HashMap<IVec2, ChunkData>,
    center_chunk: IVec2,
    loader: PlatformLoader,
    thread_count: usize,
    config: Arc<GameConfig>,
    registry: Arc<PlantRegistry>,
}

impl StreamingWorld {
    pub fn new(
        seed: u32,
        load_radius: i32,
        threads: usize,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
    ) -> anyhow::Result<Self> {
        let loader =
            PlatformLoader::new_loader(seed, threads, Arc::clone(&config), Arc::clone(&registry))?;

        // No synchronous chunk generation in this constructor — all chunks (including
        // the center) are dispatched via update(); native loaders use background
        // threads, while the wasm32 loader runs synchronously on the main thread.
        Ok(Self {
            seed,
            load_radius,
            loaded: HashMap::new(),
            center_chunk: IVec2::ZERO,
            loader,
            thread_count: threads,
            config,
            registry,
        })
    }

    pub fn update(&mut self, camera_position: Vec3, delta_store: &mut DeltaStore) {
        let landing_rules = PlantLandingRules {
            registry: &self.registry,
            biome_config: &self.config.biome,
            sea_level: self.config.sea_level,
        };
        for chunk in self.loader.poll() {
            let chunk = apply_chunk_delta(chunk, delta_store, &landing_rules);
            self.loaded.insert(chunk.coord, chunk);
        }

        self.center_chunk = world_to_chunk(camera_position);
        let required = required_coords(self.center_chunk, self.load_radius);

        self.loaded.retain(|coord, _| required.contains(coord));
        self.loader.cancel_outside(&required);

        for &coord in &required {
            if !self.loaded.contains_key(&coord) {
                self.loader.dispatch(coord, self.seed);
            }
        }
    }

    pub fn chunks(&self) -> &HashMap<IVec2, ChunkData> {
        &self.loaded
    }

    pub fn reassemble_loaded_chunk(&mut self, coord: IVec2, delta_store: &mut DeltaStore) -> bool {
        let Some(chunk) = self.loaded.get_mut(&coord) else {
            return false;
        };

        let next_plants = if let Some(delta) = delta_store.get(&coord).cloned() {
            let mut delta = delta;
            prune_chunk_delta_on_load(
                coord,
                chunk,
                &mut delta,
                &PlantLandingRules {
                    registry: &self.registry,
                    biome_config: &self.config.biome,
                    sea_level: self.config.sea_level,
                },
            );
            let plants = assemble_plants(&chunk.content.base_plants, &delta);
            if delta.is_empty() {
                let _ = delta_store.remove(&coord);
            } else {
                *delta_store.get_or_create(coord) = delta;
            }

            plants
        } else {
            chunk.content.base_plants.clone()
        };

        chunk.content.set_plants(next_plants)
    }

    pub fn tick_loaded_chunk_growth(
        &mut self,
        total_hours: f64,
        delta_store: &mut DeltaStore,
    ) -> Vec<IVec2> {
        let mut changed_coords = HashSet::new();
        let mut loaded_coords: Vec<IVec2> = self.loaded.keys().copied().collect();
        loaded_coords.sort_by_key(|coord| (coord.x, coord.y));

        let tick_context = LifecycleTickContext {
            loaded: &self.loaded,
            total_hours,
            world_seed: self.seed,
            landing_rules: PlantLandingRules {
                registry: &self.registry,
                biome_config: &self.config.biome,
                sea_level: self.config.sea_level,
            },
        };

        for coord in loaded_coords {
            tick_chunk_lifecycle(coord, &tick_context, delta_store, &mut changed_coords);
        }

        let mut changed_coords: Vec<_> = changed_coords.into_iter().collect();
        changed_coords.sort_by_key(|coord| (coord.x, coord.y));
        changed_coords
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    pub fn reload_config(&mut self, config: &GameConfig) {
        let new_config = Arc::new(config.clone());
        if let Ok(loader) = PlatformLoader::new_loader(
            self.seed,
            self.thread_count,
            Arc::clone(&new_config),
            Arc::clone(&self.registry),
        ) {
            self.loaded.clear();
            self.loader = loader;
            self.load_radius = config.world.load_radius;
            self.config = new_config;
        }
    }

    pub fn stats(&self) -> StreamingStats {
        StreamingStats {
            loaded_chunks: self.loaded.len(),
            pending_chunks: self.loader.pending_count(),
            center_chunk: self.center_chunk,
        }
    }
}

fn apply_chunk_delta(
    mut chunk: ChunkData,
    delta_store: &mut DeltaStore,
    landing_rules: &PlantLandingRules<'_>,
) -> ChunkData {
    let Some(existing) = delta_store.get(&chunk.coord).cloned() else {
        return chunk;
    };

    let mut delta = existing;
    prune_chunk_delta_on_load(chunk.coord, &chunk, &mut delta, landing_rules);
    chunk
        .content
        .set_plants(assemble_plants(&chunk.content.base_plants, &delta));

    if delta.is_empty() {
        let _ = delta_store.remove(&chunk.coord);
        return chunk;
    }

    *delta_store.get_or_create(chunk.coord) = delta;
    chunk
}

fn tick_chunk_lifecycle(
    coord: IVec2,
    context: &LifecycleTickContext<'_>,
    delta_store: &mut DeltaStore,
    changed_coords: &mut HashSet<IVec2>,
) {
    let Some(chunk) = context.loaded.get(&coord) else {
        return;
    };

    let last_sim_hour = {
        let delta = delta_store.get_or_create(coord);
        sanitize_chunk_timestamps(coord, context.total_hours, delta)
    };

    let current_hour = context.total_hours.max(last_sim_hour);
    let target_hour = current_hour.min(last_sim_hour + MAX_CATCH_UP_HOURS);
    let missed_boundaries =
        (target_hour.floor() as i64 - last_sim_hour.floor() as i64).max(0) as u64;
    if missed_boundaries == 0 {
        return;
    }

    if target_hour < current_hour {
        log::warn!(
            "clamped lifecycle catch-up for chunk {coord:?} from {:.2}h to {:.2}h (cap {:.2}h)",
            context.total_hours - last_sim_hour,
            target_hour - last_sim_hour,
            MAX_CATCH_UP_HOURS
        );
    }

    for boundary in 1..=missed_boundaries {
        let round_hour = last_sim_hour.floor() + boundary as f64;
        let mut chunk_changed = false;
        {
            let delta = delta_store.get_or_create(coord);
            let previous_stages: Vec<_> =
                delta.added_plants.iter().map(|plant| plant.stage).collect();

            for plant in &delta.added_plants {
                debug_assert!(
                    plant.born_hour <= context.total_hours,
                    "chunk {coord:?} has plant born in the future: {} > {}",
                    plant.born_hour,
                    context.total_hours,
                );
            }

            for plant in &mut delta.added_plants {
                if advance_delta_plant_growth(plant, round_hour, context.landing_rules.registry) {
                    chunk_changed = true;
                }
            }

            debug_assert!(
                delta
                    .added_plants
                    .iter()
                    .zip(previous_stages.iter())
                    .all(|(plant, previous)| plant.stage >= *previous),
                "chunk {coord:?} contained a regressed growth stage"
            );

            if delta.last_sim_hour != round_hour {
                delta.last_sim_hour = round_hour;
            }
        }

        if is_spread_hour(round_hour) {
            let source_delta = delta_store.get(&coord).cloned().unwrap_or_default();
            let source_plants = assemble_plants(&chunk.content.base_plants, &source_delta);

            for (plant_index, plant) in source_plants.iter().enumerate() {
                if plant.growth_stage != GrowthStage::Mature {
                    continue;
                }

                let Some(species) = context
                    .landing_rules
                    .registry
                    .species
                    .get(plant.species_index)
                else {
                    debug_assert!(
                        false,
                        "plant in chunk {coord:?} references invalid species index {}",
                        plant.species_index
                    );
                    continue;
                };

                if spread_roll(context.world_seed, coord, plant_index as u32)
                    >= species.placement.spread_chance.clamp(0.0, 1.0)
                {
                    continue;
                }

                let seed_count = spread_seed_count(context.world_seed, coord, plant_index as u32);
                for seed_i in 0..seed_count {
                    let request = SeedlingSpawnRequest {
                        coord,
                        plant_index: plant_index as u32,
                        seed_i,
                        source_position: plant.position,
                        species_index: plant.species_index,
                        round_hour,
                    };
                    let Some(seedling) = spawn_seedling(
                        context.world_seed,
                        &request,
                        context.landing_rules.registry,
                    ) else {
                        continue;
                    };

                    let target_coord = world_to_chunk(seedling.position);
                    if let Some(target_chunk) = context.loaded.get(&target_coord) {
                        let existing =
                            existing_plants_for_chunk(target_chunk, delta_store.get(&target_coord));
                        let Some(seedling) = validate_seedling_landing(
                            &seedling,
                            target_coord,
                            target_chunk,
                            &existing,
                            &context.landing_rules,
                        ) else {
                            continue;
                        };

                        delta_store
                            .get_or_create(target_coord)
                            .added_plants
                            .push(seedling);
                        changed_coords.insert(target_coord);
                    } else {
                        delta_store
                            .get_or_create(target_coord)
                            .added_plants
                            .push(seedling);
                    }
                    chunk_changed = true;
                }
            }
        }

        if chunk_changed {
            changed_coords.insert(coord);
        }
    }

    let final_delta = delta_store.get_or_create(coord);
    debug_assert!(
        final_delta.last_sim_hour <= context.total_hours,
        "chunk {coord:?} last_sim_hour {} exceeds total_hours {}",
        final_delta.last_sim_hour,
        context.total_hours
    );
}

fn sanitize_chunk_timestamps(coord: IVec2, total_hours: f64, delta: &mut ChunkDelta) -> f64 {
    if delta.last_sim_hour > total_hours {
        log::warn!(
            "clamping chunk {coord:?} last_sim_hour from {:.3}h down to current total_hours {:.3}h",
            delta.last_sim_hour,
            total_hours
        );
        delta.last_sim_hour = total_hours;
    }

    for plant in &mut delta.added_plants {
        if plant.born_hour > total_hours {
            log::warn!(
                "clamping future-born plant in chunk {coord:?} from {:.3}h down to current total_hours {:.3}h",
                plant.born_hour,
                total_hours
            );
            plant.born_hour = total_hours;
        }
    }

    debug_assert!(
        delta.last_sim_hour <= total_hours,
        "chunk {coord:?} last_sim_hour {} exceeds total_hours {total_hours}",
        delta.last_sim_hour
    );

    debug_assert!(
        delta.last_sim_hour <= total_hours,
        "chunk {coord:?} last_sim_hour {} exceeds total_hours {total_hours}",
        delta.last_sim_hour
    );
    debug_assert!(
        delta
            .added_plants
            .iter()
            .all(|plant| plant.born_hour <= total_hours),
        "chunk {coord:?} contains plants born after total_hours {total_hours}"
    );

    delta.last_sim_hour
}

fn is_spread_hour(hour: f64) -> bool {
    (hour.floor() as i64).rem_euclid(24) == 0
}

fn spread_roll(seed: u32, coord: IVec2, plant_index: u32) -> f32 {
    hash_to_unit_float(hash4(
        seed.wrapping_add(4001),
        coord.x as u32,
        coord.y as u32,
        plant_index,
    ))
}

fn spread_seed_count(seed: u32, coord: IVec2, plant_index: u32) -> u32 {
    1 + (hash_to_unit_float(hash4(
        seed.wrapping_add(4002),
        coord.x as u32,
        coord.y as u32,
        plant_index,
    )) * 2.0)
        .floor() as u32
}

fn spawn_seedling(
    seed: u32,
    request: &SeedlingSpawnRequest,
    registry: &PlantRegistry,
) -> Option<DeltaPlant> {
    let species = registry.species.get(request.species_index)?;
    let sub_id = request
        .plant_index
        .wrapping_mul(31)
        .wrapping_add(request.seed_i);
    let angle = hash_to_unit_float(hash4(
        seed.wrapping_add(4101),
        request.coord.x as u32,
        request.coord.y as u32,
        sub_id,
    )) * std::f32::consts::TAU;
    let distance = hash_to_unit_float(hash4(
        seed.wrapping_add(4102),
        request.coord.x as u32,
        request.coord.y as u32,
        sub_id,
    ))
    .sqrt()
        * species.placement.spread_radius.max(0.0);
    let height = species.height_range[0]
        + hash_to_unit_float(hash4(
            seed.wrapping_add(4201),
            request.coord.x as u32,
            request.coord.y as u32,
            sub_id,
        )) * (species.height_range[1] - species.height_range[0]);
    let rotation = hash_to_unit_float(hash4(
        seed.wrapping_add(4202),
        request.coord.x as u32,
        request.coord.y as u32,
        sub_id,
    )) * std::f32::consts::TAU;

    Some(DeltaPlant {
        position: Vec3::new(
            request.source_position.x + angle.cos() * distance,
            request.source_position.y,
            request.source_position.z + angle.sin() * distance,
        ),
        rotation,
        height,
        species_index: request.species_index,
        stage: GrowthStage::Seedling,
        born_hour: request.round_hour,
    })
}

fn prune_chunk_delta_on_load(
    coord: IVec2,
    chunk: &ChunkData,
    delta: &mut ChunkDelta,
    landing_rules: &PlantLandingRules<'_>,
) -> bool {
    let original_removed_len = delta.removed_base.len();
    let mut changed = delta.prune_removed_base(chunk.content.base_plants.len());
    if changed && delta.removed_base.len() != original_removed_len {
        log::info!(
            "pruned {} stale removed_base indices from chunk {coord:?} on load",
            original_removed_len - delta.removed_base.len()
        );
    }
    let mut retained = Vec::with_capacity(delta.added_plants.len());
    let mut existing = chunk.content.base_plants.clone();
    let original_added = std::mem::take(&mut delta.added_plants);
    let original_snapshot = original_added.clone();
    let original_len = original_added.len();

    for plant in original_added {
        if let Some(validated) =
            validate_seedling_landing(&plant, coord, chunk, &existing, landing_rules)
        {
            existing.push(crate::world_core::chunk::PlantInstance {
                position: validated.position,
                rotation: validated.rotation,
                height: validated.height,
                species_index: validated.species_index,
                growth_stage: validated.stage,
            });
            retained.push(validated);
        }
    }

    if retained != original_snapshot {
        changed = true;
    }

    if retained.len() != original_len {
        log::info!(
            "pruned {} invalid deferred seedlings from chunk {coord:?} on load",
            original_len - retained.len()
        );
    }

    delta.added_plants = retained;
    changed
}

fn validate_seedling_landing(
    seedling: &DeltaPlant,
    target_coord: IVec2,
    target_chunk: &ChunkData,
    existing: &[crate::world_core::chunk::PlantInstance],
    landing_rules: &PlantLandingRules<'_>,
) -> Option<DeltaPlant> {
    let Some(species) = landing_rules.registry.species.get(seedling.species_index) else {
        debug_assert!(
            false,
            "seedling references invalid species index {}",
            seedling.species_index
        );
        return None;
    };

    if target_chunk.terrain.heights.is_empty() || target_chunk.terrain.moisture.is_empty() {
        return None;
    }

    let local_x = seedling.position.x - target_coord.x as f32 * CHUNK_SIZE_METERS;
    let local_z = seedling.position.z - target_coord.y as f32 * CHUNK_SIZE_METERS;
    let terrain = &target_chunk.terrain;
    let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
    let moisture = sample_field_bilinear(&terrain.moisture, local_x, local_z);
    let slope = estimate_slope(&terrain.heights, local_x, local_z);
    let biome = classify(height, moisture, landing_rules.biome_config);

    if height < landing_rules.sea_level
        || moisture < species.placement.min_moisture
        || moisture > species.placement.max_moisture
        || height < species.placement.min_altitude
        || height > species.placement.max_altitude
        || slope > species.placement.max_slope
        || !species
            .placement
            .biomes
            .iter()
            .any(|candidate| candidate == biome_name(biome))
    {
        return None;
    }

    let spacing = min_spacing_for_species(&species.kind);
    let mut landed = seedling.clone();
    landed.position.y = height;

    if existing
        .iter()
        .any(|plant| plant.position.distance(landed.position) < spacing)
    {
        return None;
    }

    Some(landed)
}

fn existing_plants_for_chunk(chunk: &ChunkData, delta: Option<&ChunkDelta>) -> Vec<PlantInstance> {
    delta
        .map(|delta| assemble_plants(&chunk.content.base_plants, delta))
        .unwrap_or_else(|| chunk.content.base_plants.clone())
}

fn min_spacing_for_species(kind: &str) -> f32 {
    if kind == "shrub" {
        3.0
    } else {
        8.0
    }
}

fn biome_name(biome: Biome) -> &'static str {
    match biome {
        Biome::Forest => "Forest",
        Biome::Grassland => "Grassland",
        Biome::Desert => "Desert",
        Biome::Rock => "Rock",
        Biome::Snow => "Snow",
    }
}

fn world_to_chunk(position: Vec3) -> IVec2 {
    IVec2::new(
        (position.x / CHUNK_SIZE_METERS).floor() as i32,
        (position.z / CHUNK_SIZE_METERS).floor() as i32,
    )
}

fn required_coords(center: IVec2, radius: i32) -> HashSet<IVec2> {
    let width = (radius * 2 + 1).max(1);
    let mut required = HashSet::with_capacity((width * width) as usize);

    for z in -radius..=radius {
        for x in -radius..=radius {
            required.insert(IVec2::new(center.x + x, center.y + z));
        }
    }

    required
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::{IVec2, Vec3};

    use super::{apply_chunk_delta, PlantLandingRules, StreamingWorld};
    use crate::world_core::chunk::{
        ChunkContent, ChunkData, ChunkTerrain, PlantInstance, CHUNK_GRID_RESOLUTION,
        CHUNK_SIZE_METERS,
    };
    use crate::world_core::lifecycle::{ChunkDelta, GrowthStage};
    use crate::world_core::{
        config::GameConfig,
        herbarium::{Herbarium, PlantRegistry},
    };
    use crate::world_runtime::DeltaStore;

    fn test_registry() -> Arc<PlantRegistry> {
        Arc::new(PlantRegistry::from_herbarium(&Herbarium::default_seeded()))
    }

    fn test_chunk(coord: IVec2, plants: Vec<PlantInstance>) -> ChunkData {
        ChunkData {
            coord,
            terrain: ChunkTerrain {
                heights: Vec::new(),
                moisture: Vec::new(),
                min_height: 0.0,
                max_height: 0.0,
                has_water: false,
            },
            content: ChunkContent {
                base_plants: plants.clone(),
                plants,
                plants_revision: 0,
                houses: Vec::new(),
            },
        }
    }

    fn flat_terrain(height: f32, moisture: f32) -> ChunkTerrain {
        let total = CHUNK_GRID_RESOLUTION * CHUNK_GRID_RESOLUTION;
        ChunkTerrain {
            heights: vec![height; total],
            moisture: vec![moisture; total],
            min_height: height,
            max_height: height,
            has_water: height < 40.0,
        }
    }

    fn test_chunk_with_terrain(coord: IVec2, plants: Vec<PlantInstance>) -> ChunkData {
        ChunkData {
            coord,
            terrain: flat_terrain(80.0, 0.75),
            content: ChunkContent {
                base_plants: plants.clone(),
                plants,
                plants_revision: 0,
                houses: Vec::new(),
            },
        }
    }

    #[test]
    fn apply_chunk_delta_prunes_stale_removed_base_indices_on_load() {
        let coord = IVec2::new(4, 5);
        let base_plants = vec![
            PlantInstance {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: 0.0,
                height: 10.0,
                species_index: 0,
                growth_stage: GrowthStage::Mature,
            },
            PlantInstance {
                position: Vec3::new(4.0, 5.0, 6.0),
                rotation: 0.0,
                height: 11.0,
                species_index: 0,
                growth_stage: GrowthStage::Mature,
            },
        ];
        let chunk = ChunkData {
            coord,
            terrain: ChunkTerrain {
                heights: Vec::new(),
                moisture: Vec::new(),
                min_height: 0.0,
                max_height: 0.0,
                has_water: false,
            },
            content: ChunkContent {
                base_plants: base_plants.clone(),
                plants: base_plants,
                plants_revision: 0,
                houses: Vec::new(),
            },
        };
        let mut deltas = DeltaStore::default();
        *deltas.get_or_create(coord) = ChunkDelta {
            removed_base: vec![1, 4],
            added_plants: Vec::new(),
            last_sim_hour: 0.0,
        };

        let registry = test_registry();
        let config = GameConfig::default();
        let chunk = apply_chunk_delta(
            chunk,
            &mut deltas,
            &PlantLandingRules {
                registry: registry.as_ref(),
                biome_config: &config.biome,
                sea_level: config.sea_level,
            },
        );
        let delta = deltas.get(&coord).expect("delta should remain");

        assert_eq!(delta.removed_base, vec![1]);
        assert_eq!(chunk.content.plants.len(), 1);
        assert_eq!(chunk.content.plants[0].position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(chunk.content.plants_revision, 1);
    }

    #[test]
    fn apply_chunk_delta_prunes_invalid_deferred_seedlings_idempotently() {
        let coord = IVec2::new(1, -2);
        let config = GameConfig::default();
        let registry = test_registry();
        let valid_x = coord.x as f32 * CHUNK_SIZE_METERS + 24.0;
        let valid_z = coord.y as f32 * CHUNK_SIZE_METERS + 36.0;
        let valid_seedling = crate::world_core::lifecycle::DeltaPlant {
            position: Vec3::new(valid_x, 12.0, valid_z),
            rotation: 0.3,
            height: 8.0,
            species_index: 0,
            stage: GrowthStage::Seedling,
            born_hour: 24.0,
        };
        let duplicate_seedling = crate::world_core::lifecycle::DeltaPlant {
            position: Vec3::new(valid_x, 999.0, valid_z),
            ..valid_seedling.clone()
        };

        let mut deltas = DeltaStore::default();
        *deltas.get_or_create(coord) = ChunkDelta {
            removed_base: Vec::new(),
            added_plants: vec![valid_seedling, duplicate_seedling],
            last_sim_hour: 24.0,
        };

        let first = apply_chunk_delta(
            test_chunk_with_terrain(coord, Vec::new()),
            &mut deltas,
            &PlantLandingRules {
                registry: registry.as_ref(),
                biome_config: &config.biome,
                sea_level: config.sea_level,
            },
        );
        let second = apply_chunk_delta(
            test_chunk_with_terrain(coord, Vec::new()),
            &mut deltas,
            &PlantLandingRules {
                registry: registry.as_ref(),
                biome_config: &config.biome,
                sea_level: config.sea_level,
            },
        );
        let delta = deltas.get(&coord).expect("delta should remain");

        assert_eq!(delta.added_plants.len(), 1);
        assert_eq!(delta.added_plants[0].position.y, 80.0);
        assert_eq!(first.content.plants.len(), 1);
        assert_eq!(second.content.plants.len(), 1);
        assert_eq!(
            first.content.plants[0].position,
            second.content.plants[0].position
        );
        assert_eq!(
            first.content.plants[0].growth_stage,
            second.content.plants[0].growth_stage
        );
    }

    #[test]
    fn tick_loaded_chunk_growth_advances_delta_stage_and_reports_changed_chunk() {
        let registry = test_registry();
        let mut streaming = StreamingWorld::new(
            42,
            1,
            1,
            Arc::new(GameConfig::default()),
            Arc::clone(&registry),
        )
        .expect("streaming world should build");
        let coord = IVec2::new(2, 3);
        streaming
            .loaded
            .insert(coord, test_chunk(coord, Vec::new()));

        let mut deltas = DeltaStore::default();
        *deltas.get_or_create(coord) = ChunkDelta {
            removed_base: Vec::new(),
            added_plants: vec![crate::world_core::lifecycle::DeltaPlant {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: 0.0,
                height: 8.0,
                species_index: 0,
                stage: GrowthStage::Seedling,
                born_hour: 100.0,
            }],
            last_sim_hour: 100.0,
        };

        let changed = streaming.tick_loaded_chunk_growth(148.0, &mut deltas);
        let delta = deltas.get(&coord).expect("delta should exist");

        assert_eq!(changed, vec![coord]);
        assert_eq!(delta.added_plants[0].stage, GrowthStage::Young);
        assert_eq!(delta.last_sim_hour, 148.0);
    }

    #[test]
    fn tick_loaded_chunk_growth_clamps_large_catch_up_gaps() {
        let registry = test_registry();
        let mut streaming =
            StreamingWorld::new(42, 1, 1, Arc::new(GameConfig::default()), registry)
                .expect("streaming world should build");
        let coord = IVec2::new(-1, 4);
        streaming
            .loaded
            .insert(coord, test_chunk(coord, Vec::new()));

        let mut deltas = DeltaStore::default();
        *deltas.get_or_create(coord) = ChunkDelta {
            removed_base: Vec::new(),
            added_plants: vec![crate::world_core::lifecycle::DeltaPlant {
                position: Vec3::ZERO,
                rotation: 0.0,
                height: 8.0,
                species_index: 0,
                stage: GrowthStage::Mature,
                born_hour: 0.0,
            }],
            last_sim_hour: 0.0,
        };

        let changed = streaming.tick_loaded_chunk_growth(900.0, &mut deltas);
        let delta = deltas.get(&coord).expect("delta should exist");

        assert!(changed.is_empty());
        assert_eq!(delta.last_sim_hour, 500.0);
        assert_eq!(delta.added_plants[0].stage, GrowthStage::Mature);
    }

    #[test]
    fn tick_loaded_chunk_growth_clamps_future_timestamps_before_simulating() {
        let registry = test_registry();
        let mut streaming =
            StreamingWorld::new(42, 1, 1, Arc::new(GameConfig::default()), registry)
                .expect("streaming world should build");
        let coord = IVec2::new(-3, -2);
        streaming
            .loaded
            .insert(coord, test_chunk_with_terrain(coord, Vec::new()));

        let mut deltas = DeltaStore::default();
        *deltas.get_or_create(coord) = ChunkDelta {
            removed_base: Vec::new(),
            added_plants: vec![crate::world_core::lifecycle::DeltaPlant {
                position: Vec3::new(
                    coord.x as f32 * CHUNK_SIZE_METERS + 12.0,
                    80.0,
                    coord.y as f32 * CHUNK_SIZE_METERS + 18.0,
                ),
                rotation: 0.0,
                height: 8.0,
                species_index: 0,
                stage: GrowthStage::Seedling,
                born_hour: 7.0,
            }],
            last_sim_hour: 6.000931811249009,
        };

        let changed = streaming.tick_loaded_chunk_growth(5.5, &mut deltas);
        let delta = deltas.get(&coord).expect("delta should exist");

        assert!(changed.is_empty());
        assert_eq!(delta.last_sim_hour, 5.5);
        assert_eq!(delta.added_plants[0].born_hour, 5.5);
        assert_eq!(delta.added_plants[0].stage, GrowthStage::Seedling);
    }

    #[test]
    fn tick_loaded_chunk_growth_spreads_deterministically_in_loaded_chunks() {
        let registry = test_registry();
        let config = Arc::new(GameConfig::default());
        let mut left = StreamingWorld::new(42, 1, 1, Arc::clone(&config), Arc::clone(&registry))
            .expect("streaming world should build");
        let mut right =
            StreamingWorld::new(42, 1, 1, config, registry).expect("streaming world should build");

        let coord = (-4..=4)
            .flat_map(|z| (-4..=4).map(move |x| IVec2::new(x, z)))
            .find(|coord| super::spread_roll(42, *coord, 0) < 0.3)
            .expect("expected a coord with a successful spread roll");
        let base_plant = PlantInstance {
            position: Vec3::new(
                coord.x as f32 * CHUNK_SIZE_METERS + CHUNK_SIZE_METERS * 0.5,
                80.0,
                coord.y as f32 * CHUNK_SIZE_METERS + CHUNK_SIZE_METERS * 0.5,
            ),
            rotation: 0.0,
            height: 12.0,
            species_index: 0,
            growth_stage: GrowthStage::Mature,
        };

        left.loaded.insert(
            coord,
            test_chunk_with_terrain(coord, vec![base_plant.clone()]),
        );
        right
            .loaded
            .insert(coord, test_chunk_with_terrain(coord, vec![base_plant]));

        let mut left_deltas = DeltaStore::default();
        let mut right_deltas = DeltaStore::default();

        let left_changed = left.tick_loaded_chunk_growth(24.0, &mut left_deltas);
        let right_changed = right.tick_loaded_chunk_growth(24.0, &mut right_deltas);
        let left_delta = left_deltas.get(&coord).expect("left delta should exist");
        let right_delta = right_deltas.get(&coord).expect("right delta should exist");

        assert_eq!(left_changed, right_changed);
        assert_eq!(left_delta.last_sim_hour, 24.0);
        assert_eq!(right_delta.last_sim_hour, 24.0);
        assert!(!left_delta.added_plants.is_empty());
        assert_eq!(
            left_delta.added_plants.len(),
            right_delta.added_plants.len()
        );

        for (left_plant, right_plant) in left_delta
            .added_plants
            .iter()
            .zip(right_delta.added_plants.iter())
        {
            assert!((left_plant.position - right_plant.position).length() < 1e-5);
            assert!((left_plant.rotation - right_plant.rotation).abs() < 1e-5);
            assert!((left_plant.height - right_plant.height).abs() < 1e-5);
            assert_eq!(left_plant.species_index, right_plant.species_index);
            assert_eq!(left_plant.stage, GrowthStage::Seedling);
            assert_eq!(left_plant.born_hour, 24.0);
        }
    }

    #[test]
    fn tick_loaded_chunk_growth_creates_deferred_seedlings_in_unloaded_target_chunks() {
        let registry = test_registry();
        let config = Arc::new(GameConfig::default());
        let mut streaming =
            StreamingWorld::new(42, 1, 1, Arc::clone(&config), Arc::clone(&registry))
                .expect("streaming world should build");

        let (coord, seedling) = (-6..=6)
            .flat_map(|z| (-6..=6).map(move |x| IVec2::new(x, z)))
            .find_map(|coord| {
                if super::spread_roll(42, coord, 0) >= 0.3 {
                    return None;
                }

                let source_position = Vec3::new(
                    coord.x as f32 * CHUNK_SIZE_METERS + CHUNK_SIZE_METERS - 2.0,
                    80.0,
                    coord.y as f32 * CHUNK_SIZE_METERS + CHUNK_SIZE_METERS * 0.5,
                );

                (0..2).find_map(|seed_i| {
                    let request = super::SeedlingSpawnRequest {
                        coord,
                        plant_index: 0,
                        seed_i,
                        source_position,
                        species_index: 0,
                        round_hour: 24.0,
                    };
                    let seedling = super::spawn_seedling(42, &request, registry.as_ref())?;
                    (super::world_to_chunk(seedling.position) != coord).then_some((coord, seedling))
                })
            })
            .expect("expected a spread that crosses into an unloaded chunk");

        let base_plant = PlantInstance {
            position: Vec3::new(
                coord.x as f32 * CHUNK_SIZE_METERS + CHUNK_SIZE_METERS - 2.0,
                80.0,
                coord.y as f32 * CHUNK_SIZE_METERS + CHUNK_SIZE_METERS * 0.5,
            ),
            rotation: 0.0,
            height: 12.0,
            species_index: 0,
            growth_stage: GrowthStage::Mature,
        };
        let target_coord = super::world_to_chunk(seedling.position);

        streaming
            .loaded
            .insert(coord, test_chunk_with_terrain(coord, vec![base_plant]));

        let mut deltas = DeltaStore::default();
        let changed = streaming.tick_loaded_chunk_growth(24.0, &mut deltas);
        let target_delta = deltas
            .get(&target_coord)
            .expect("deferred target delta should be created");

        assert!(changed.contains(&coord));
        assert!(!streaming.loaded.contains_key(&target_coord));
        assert!(target_delta
            .added_plants
            .iter()
            .any(|plant| plant.position == seedling.position));
        assert_eq!(target_delta.added_plants[0].born_hour, 24.0);
    }
}
