use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{IVec2, Vec3};

use super::chunk_loader::{ChunkLoader, PlatformLoader};
use super::delta_store::DeltaStore;
use super::lifecycle_sim::{tick_chunk_lifecycle, LifecycleTickContext};
use super::plant_landing::{prune_chunk_delta_on_load, PlantLandingRules};
use crate::world_core::chunk::{ChunkData, CHUNK_SIZE_METERS};
use crate::world_core::config::GameConfig;
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::lifecycle::assemble_plants;

pub struct StreamingStats {
    pub loaded_chunks: usize,
    pub pending_chunks: usize,
    pub center_chunk: IVec2,
}

// ---------------------------------------------------------------------------
// StreamingWorld — unified orchestration, delegates loading to PlatformLoader
// ---------------------------------------------------------------------------

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

pub(super) fn world_to_chunk(position: Vec3) -> IVec2 {
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

    use super::{apply_chunk_delta, StreamingWorld};
    use crate::world_core::chunk::{
        canonical_chunk, ChunkContent, ChunkData, ChunkTerrain, PlantInstance,
        CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS,
    };
    use crate::world_core::lifecycle::{ChunkDelta, GrowthStage};
    use crate::world_core::{
        config::GameConfig,
        herbarium::{Herbarium, PlantRegistry},
    };
    use crate::world_runtime::lifecycle_sim::{spawn_seedling, spread_roll, SeedlingSpawnRequest};
    use crate::world_runtime::plant_landing::PlantLandingRules;
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
    fn delta_persists_across_a_world_lap_and_rebases_into_the_loaded_chunk() {
        // A delta authored at a canonical chunk must reappear when that same
        // canonical chunk is loaded one full world lap away, with its plant
        // rebased into the loaded chunk's world span (M1's "full lap returnable").
        let config = GameConfig::default();
        let registry = test_registry();
        let canon = IVec2::new(4, 5);
        let raw = IVec2::new(canon.x + WORLD_SIZE_CHUNKS, canon.y);
        let lap = WORLD_SIZE_CHUNKS as f32 * CHUNK_SIZE_METERS;

        // Author the delta in the canonical chunk's span.
        let local_x = 24.0;
        let local_z = 36.0;
        let mut deltas = DeltaStore::default();
        *deltas.get_or_create(canon) = ChunkDelta {
            removed_base: Vec::new(),
            added_plants: vec![crate::world_core::lifecycle::DeltaPlant {
                position: Vec3::new(
                    canon.x as f32 * CHUNK_SIZE_METERS + local_x,
                    12.0,
                    canon.y as f32 * CHUNK_SIZE_METERS + local_z,
                ),
                rotation: 0.3,
                height: 8.0,
                species_index: 0,
                stage: GrowthStage::Mature,
                born_hour: 0.0,
            }],
            last_sim_hour: 0.0,
        };

        // Load the *same canonical chunk* a lap east and apply the delta.
        let chunk = apply_chunk_delta(
            test_chunk_with_terrain(raw, Vec::new()),
            &mut deltas,
            &PlantLandingRules {
                registry: registry.as_ref(),
                biome_config: &config.biome,
                sea_level: config.sea_level,
            },
        );

        // The plant survived the lap and now sits in the loaded chunk's span.
        assert_eq!(chunk.content.plants.len(), 1);
        let plant = &chunk.content.plants[0];
        assert!((plant.position.x - (raw.x as f32 * CHUNK_SIZE_METERS + local_x)).abs() < 1e-2);
        assert!((plant.position.z - (raw.y as f32 * CHUNK_SIZE_METERS + local_z)).abs() < 1e-2);

        // The stored delta is keyed canonically and now holds the rebased position.
        let stored = deltas
            .get(&canon)
            .expect("delta should persist canonically");
        assert_eq!(stored.added_plants.len(), 1);
        assert!(
            (stored.added_plants[0].position.x
                - (canon.x as f32 * CHUNK_SIZE_METERS + local_x + lap))
                .abs()
                < 1e-2
        );
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

        // Spread is hashed on the canonical chunk id, so predict with it too.
        let coord = (-4..=4)
            .flat_map(|z| (-4..=4).map(move |x| IVec2::new(x, z)))
            .find(|coord| spread_roll(42, canonical_chunk(*coord), 0) < 0.3)
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
                // Spread randomness is keyed on the canonical chunk id.
                let canon = canonical_chunk(coord);
                if spread_roll(42, canon, 0) >= 0.3 {
                    return None;
                }

                let source_position = Vec3::new(
                    coord.x as f32 * CHUNK_SIZE_METERS + CHUNK_SIZE_METERS - 2.0,
                    80.0,
                    coord.y as f32 * CHUNK_SIZE_METERS + CHUNK_SIZE_METERS * 0.5,
                );

                (0..2).find_map(|seed_i| {
                    let request = SeedlingSpawnRequest {
                        coord: canon,
                        plant_index: 0,
                        seed_i,
                        source_position,
                        species_index: 0,
                        round_hour: 24.0,
                    };
                    let seedling = spawn_seedling(42, &request, registry.as_ref())?;
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
        let _changed = streaming.tick_loaded_chunk_growth(24.0, &mut deltas);
        let target_delta = deltas
            .get(&target_coord)
            .expect("deferred target delta should be created");

        // The source chunk ticked (its delta clock advanced to this hour)...
        assert_eq!(
            deltas.get(&coord).map(|delta| delta.last_sim_hour),
            Some(24.0)
        );
        // ...and a seedling that crossed into an unloaded neighbour was stored
        // there deferred (no landing validation, position kept verbatim).
        assert!(!streaming.loaded.contains_key(&target_coord));
        assert!(target_delta
            .added_plants
            .iter()
            .any(|plant| plant.position == seedling.position));
        assert_eq!(target_delta.added_plants[0].born_hour, 24.0);
    }
}
