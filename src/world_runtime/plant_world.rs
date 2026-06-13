//! Resident, whole-world plant store.
//!
//! Iteration 1 of the live simulation keeps every plant in every one of the
//! `WORLD_SIZE_CHUNKS²` canonical chunks resident and simulates them on one
//! global clock — see [`docs/LIVE_SIM_ITERATION_1.md`]. This module is the
//! store: a packed `Plant` per canonical chunk, generated once at world
//! creation, read by the render bridge, ticked for growth + spread on the global
//! clock, and persisted as a compact spread-delta on top of the regenerable base.

use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

use glam::{IVec2, Vec2, Vec3};

use crate::world_core::biome::{classify, Biome};
use crate::world_core::chunk::{
    canonical_chunk, ChunkTerrain, PlantInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
    WORLD_SIZE_CHUNKS, WORLD_SIZE_METERS,
};
use crate::world_core::chunk_generator::ChunkGenerator;
use crate::world_core::config::{BiomeConfig, GameConfig, HousesConfig};
use crate::world_core::content::place_houses;
use crate::world_core::content::sampling::{hash4, hash_to_unit_float};
use crate::world_core::evolution::{
    evaluate_phenotype, inherit_and_mutate, initial_genes, normalize_altitude,
    EvolutionOverlayMode, FounderKey, MutationKey, PlantEnvironment, PlantGenes,
};
use crate::world_core::heightmap::Heightmap;
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::lifecycle::GrowthStage;
use crate::world_core::rivers::{RiverField, MAX_PLANTABLE_WETNESS};

/// Packed per-plant record — **24 bytes** (`repr(C)`), validated against the
/// real plant count in the M0 feasibility spike.
///
/// Position is stored **chunk-local and quantized** (`u16` over the 256 m chunk,
/// ~3.9 mm step), which makes the store lap-agnostic: a plant renders at any raw
/// chunk by adding that chunk's world origin, so there is no absolute position
/// frozen to one world lap. The ground height `y` is not stored — it is resampled
/// from the loaded chunk's terrain, exactly as base placement computed it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plant {
    /// Chunk-local x, quantized over `[0, CHUNK_SIZE_METERS)`.
    pub local_x: u16,
    /// Chunk-local z, quantized over `[0, CHUNK_SIZE_METERS)`.
    pub local_z: u16,
    /// Plant height, quantized over `[0, CHUNK_SIZE_METERS)`.
    pub height: u16,
    /// Y-rotation, quantized over `[0, TAU)`.
    pub rotation: u16,
    pub species: u8,
    pub stage: u8,
    pub born_hour: f32,
    pub genes: PlantGenes,
}

const MATURE: u8 = stage_to_u8(GrowthStage::Mature);
const DEAD: u8 = stage_to_u8(GrowthStage::Dead);

const fn stage_to_u8(stage: GrowthStage) -> u8 {
    match stage {
        GrowthStage::Seedling => 0,
        GrowthStage::Young => 1,
        GrowthStage::Mature => 2,
        GrowthStage::Dead => 3,
    }
}

fn stage_from_u8(value: u8) -> GrowthStage {
    match value {
        0 => GrowthStage::Seedling,
        1 => GrowthStage::Young,
        3 => GrowthStage::Dead,
        _ => GrowthStage::Mature,
    }
}

// Quantize `[0, CHUNK_SIZE_METERS)` onto the `u16` range using a 2^16 divisor and
// `floor`, so the dequantized value is always strictly less than
// `CHUNK_SIZE_METERS`. A plant therefore never reconstructs exactly on the chunk
// boundary, where `floor(pos / CHUNK_SIZE_METERS)` would attribute it to the
// neighbouring chunk.
const SPAN_STEPS: f32 = 65536.0;

/// Sim-hours per global spread (reproduction) round — one pass per sim-day. The
/// runtime drives the cadence and runs catch-up passes at this granularity, and
/// [`day_bucket`] salts each round's roll with `floor(born_hour / this)`, so the
/// width must be shared. Owned here as the single source of truth.
pub const SPREAD_TICK_HOURS: f64 = 24.0;

fn quantize_span(value: f32) -> u16 {
    (value / CHUNK_SIZE_METERS * SPAN_STEPS)
        .floor()
        .clamp(0.0, u16::MAX as f32) as u16
}

fn dequantize_span(value: u16) -> f32 {
    value as f32 / SPAN_STEPS * CHUNK_SIZE_METERS
}

fn sample_chunk_field(field: &[f32], local_x: f32, local_z: f32) -> f32 {
    let max = (CHUNK_GRID_RESOLUTION - 1) as f32;
    let gx = (local_x / CHUNK_SIZE_METERS * max).clamp(0.0, max);
    let gz = (local_z / CHUNK_SIZE_METERS * max).clamp(0.0, max);
    let x0 = gx.floor() as usize;
    let z0 = gz.floor() as usize;
    let x1 = (x0 + 1).min(CHUNK_GRID_RESOLUTION - 1);
    let z1 = (z0 + 1).min(CHUNK_GRID_RESOLUTION - 1);
    let tx = gx - x0 as f32;
    let tz = gz - z0 as f32;
    let idx = |x: usize, z: usize| z * CHUNK_GRID_RESOLUTION + x;
    let a = field[idx(x0, z0)] * (1.0 - tx) + field[idx(x1, z0)] * tx;
    let b = field[idx(x0, z1)] * (1.0 - tx) + field[idx(x1, z1)] * tx;
    a * (1.0 - tz) + b * tz
}

fn plant_environment(
    heightmap: &Heightmap,
    rivers: &RiverField,
    sea_level: f32,
    world_x: f32,
    world_z: f32,
) -> PlantEnvironment {
    let height = heightmap.sample_height(world_x, world_z);
    PlantEnvironment {
        moisture: heightmap.sample_moisture(world_x, world_z),
        altitude: normalize_altitude(height, sea_level),
        river_wetness: rivers.wetness(world_x, world_z),
        shade: 0.0,
        root_pressure: 0.0,
    }
}

fn plant_life_environment(
    heightmap: &Heightmap,
    rivers: &RiverField,
    sea_level: f32,
    chunk_idx: usize,
    plant: &Plant,
) -> PlantEnvironment {
    let n = WORLD_SIZE_CHUNKS;
    let cx = (chunk_idx as i32) % n;
    let cz = (chunk_idx as i32) / n;
    let world_x = cx as f32 * CHUNK_SIZE_METERS + dequantize_span(plant.local_x);
    let world_z = cz as f32 * CHUNK_SIZE_METERS + dequantize_span(plant.local_z);
    plant_environment(heightmap, rivers, sea_level, world_x, world_z)
}

fn overlay_tint(
    mode: EvolutionOverlayMode,
    plant: &Plant,
    phenotype: crate::world_core::evolution::PlantPhenotype,
) -> [f32; 4] {
    match mode {
        EvolutionOverlayMode::Off => phenotype.leaf_tint,
        EvolutionOverlayMode::WetPreference => heat_tint(PlantGenes::unit(plant.genes.wet_pref)),
        EvolutionOverlayMode::AltitudePreference => {
            heat_tint(PlantGenes::unit(plant.genes.alt_pref))
        }
        EvolutionOverlayMode::AbioticFitness => fitness_tint(phenotype.abiotic_fitness),
        EvolutionOverlayMode::CompetitionStress => stress_tint(phenotype.stress),
        EvolutionOverlayMode::Generation => {
            if plant.born_hour <= 0.0 {
                [0.18, 0.18, 0.22, 1.0]
            } else {
                heat_tint(((plant.born_hour / (24.0 * 30.0)).ln_1p() / 4.0).clamp(0.0, 1.0))
            }
        }
    }
}

fn heat_tint(value: f32) -> [f32; 4] {
    let t = value.clamp(0.0, 1.0);
    [
        0.15 + t * 1.05,
        0.25 + (1.0 - (t - 0.5).abs() * 2.0).max(0.0) * 0.75,
        1.15 - t,
        1.0,
    ]
}

fn fitness_tint(value: f32) -> [f32; 4] {
    let t = value.clamp(0.0, 1.0);
    [1.1 - t * 0.85, 0.25 + t * 0.9, 0.2, 1.0]
}

fn stress_tint(value: f32) -> [f32; 4] {
    let t = value.clamp(0.0, 1.0);
    [0.3 + t * 0.9, 0.95 - t * 0.65, 0.25, 1.0]
}

#[derive(Default)]
struct PhenotypeSums {
    abiotic_fitness: f64,
    competition_room: f64,
    stress: f64,
    height_scale: f64,
    width_scale: f64,
    maturity_scale: f64,
    lifespan_scale: f64,
    seed_count_scale: f64,
    spread_radius_scale: f64,
    establishment_chance: f64,
}

impl PhenotypeSums {
    fn add(&mut self, p: crate::world_core::evolution::PlantPhenotype) {
        self.abiotic_fitness += p.abiotic_fitness as f64;
        self.competition_room += p.competition_room as f64;
        self.stress += p.stress as f64;
        self.height_scale += p.height_scale as f64;
        self.width_scale += p.width_scale as f64;
        self.maturity_scale += p.maturity_scale as f64;
        self.lifespan_scale += p.lifespan_scale as f64;
        self.seed_count_scale += p.seed_count_scale as f64;
        self.spread_radius_scale += p.spread_radius_scale as f64;
        self.establishment_chance += p.establishment_chance as f64;
    }

    fn mean_json(&self, count: usize) -> serde_json::Value {
        let n = count.max(1) as f64;
        serde_json::json!({
            "abiotic_fitness": self.abiotic_fitness / n,
            "competition_room": self.competition_room / n,
            "stress": self.stress / n,
            "height_scale": self.height_scale / n,
            "width_scale": self.width_scale / n,
            "maturity_scale": self.maturity_scale / n,
            "lifespan_scale": self.lifespan_scale / n,
            "seed_count_scale": self.seed_count_scale / n,
            "spread_radius_scale": self.spread_radius_scale / n,
            "establishment_chance": self.establishment_chance / n,
        })
    }

    fn mean_abiotic_fitness(&self, count: usize) -> f64 {
        self.abiotic_fitness / count.max(1) as f64
    }

    fn mean_stress(&self, count: usize) -> f64 {
        self.stress / count.max(1) as f64
    }
}

fn torus_delta(a: f32, b: f32, world: f32) -> f32 {
    let d = a - b;
    if d > world * 0.5 {
        d - world
    } else if d < -world * 0.5 {
        d + world
    } else {
        d
    }
}

fn quantize_rotation(radians: f32) -> u16 {
    (radians.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * u16::MAX as f32)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16
}

impl Plant {
    /// Pack a generated plant whose absolute position lies in the chunk whose
    /// world origin is `(origin_x, origin_z)`.
    ///
    /// Base plants are snapped to the **base-snapshot storage grid** here, so a
    /// world generated locally is bit-identical to one rebuilt from a downloaded
    /// `world_base.bin`: positions to a 10-bit-per-axis grid (`POS_BITS`, 0.25 m),
    /// height to `pack_height` (0.125 m), rotation to 8 bits (~1.4°). Without this
    /// the snapshot would be lossy relative to generation and the two paths would
    /// diverge — and since spread RNG is keyed on a plant's index within its
    /// chunk, the caller also Morton-sorts each chunk after packing.
    fn pack(
        plant: &PlantInstance,
        origin_x: f32,
        origin_z: f32,
        seed: u32,
        canon: IVec2,
        terrain: &ChunkTerrain,
        sea_level: f32,
    ) -> Self {
        let h8 = pack_height(plant.height);
        let r8 = pack_rotation(plant.rotation);
        let local_x = snap_pos(quantize_span(plant.position.x - origin_x));
        let local_z = snap_pos(quantize_span(plant.position.z - origin_z));
        let env = PlantEnvironment {
            moisture: sample_chunk_field(
                &terrain.moisture,
                dequantize_span(local_x),
                dequantize_span(local_z),
            ),
            altitude: normalize_altitude(plant.position.y, sea_level),
            river_wetness: sample_chunk_field(
                &terrain.river,
                dequantize_span(local_x),
                dequantize_span(local_z),
            ),
            shade: 0.0,
            root_pressure: 0.0,
        };
        let genes = initial_genes(
            seed,
            FounderKey {
                chunk: canon,
                local_x,
                local_z,
                species: plant.species_index as u8,
            },
            env,
        );
        Plant {
            local_x,
            local_z,
            height: unpack_height(h8),
            rotation: (r8 as u16) << 8,
            species: plant.species_index as u8,
            stage: stage_to_u8(plant.growth_stage),
            born_hour: 0.0,
            genes,
        }
    }

    /// Reconstruct a renderable instance for the raw chunk at `raw_coord`,
    /// resampling ground height from that chunk's terrain.
    fn to_instance(self, raw_coord: IVec2, terrain: &ChunkTerrain) -> PlantInstance {
        let local_x = dequantize_span(self.local_x);
        let local_z = dequantize_span(self.local_z);
        let y = terrain.height_at_world(local_x, local_z);
        PlantInstance {
            position: Vec3::new(
                raw_coord.x as f32 * CHUNK_SIZE_METERS + local_x,
                y,
                raw_coord.y as f32 * CHUNK_SIZE_METERS + local_z,
            ),
            rotation: self.rotation as f32 / u16::MAX as f32 * std::f32::consts::TAU,
            height: dequantize_span(self.height),
            species_index: self.species as usize,
            growth_stage: stage_from_u8(self.stage),
            height_scale: 1.0,
            width_scale: 1.0,
            stress: 0.0,
            leaf_tint: [1.0, 1.0, 1.0, 1.0],
            decay: 0.0,
        }
    }
}

/// Per-canonical-chunk packed plant lists for the whole finite world.
pub struct PlantWorld {
    /// One list per canonical chunk, indexed `cz * WORLD_SIZE_CHUNKS + cx`.
    chunks: Vec<Vec<Plant>>,
    /// Count of plants not yet `Mature` (dead snags excluded), per chunk —
    /// feeds the spread pass's saturation logic.
    immature: Vec<u32>,
    /// Earliest sim-hour at which any plant in the chunk crosses a life
    /// threshold (young/mature/dead/despawn) — lets the growth tick skip
    /// settled chunks without freezing mature plants short of their death.
    /// `0.0` forces a recompute on the next pass; `INFINITY` means nothing left
    /// to happen (empty, or all-mature immortals).
    next_event: Vec<f64>,
    /// Sim-hour of the most recent growth pass; snag decay (the render-side
    /// shrink) is measured against this.
    growth_hours: f64,
    /// Per chunk: all-mature and saturated — the last spread pass added nothing
    /// here. A chunk's plants can only spread into the 8 adjacent chunks (spread
    /// radius ≪ chunk size), so phase 1 skips a chunk only when it *and* all eight
    /// neighbours are saturated. That keeps a packed chunk feeding a still-filling
    /// neighbour, while making the global pass cheap once a whole region fills.
    saturated: Vec<bool>,
    /// Number of deterministic base-flora plants per chunk, always the prefix of
    /// each chunk's list. Persistence saves only the spread suffix beyond this,
    /// since the base is regenerable from the seed.
    base_count: Vec<u32>,
    /// Biome at each chunk's centre, for per-biome telemetry.
    biome: Vec<Biome>,
    /// Per-chunk overview-map cell byte (biome + water override), the same value
    /// fed to the loading visualization. Retained so the base-world snapshot can
    /// repaint the map on a cache load without regenerating terrain.
    cell_bytes: Vec<u8>,
    registry: Arc<PlantRegistry>,
    /// Terrain + rules for validating spread landings without the full per-chunk
    /// grid: the heightmap is point-sampled at each seedling position.
    heightmap: Heightmap,
    /// Global river field, retained so the spread landing pass can reject
    /// seedlings that fall in a river channel — the same guard base flora
    /// applies via the per-chunk `terrain.river` grid.
    rivers: Arc<RiverField>,
    /// House XZ positions per canonical chunk (`cz * WORLD_SIZE_CHUNKS + cx`),
    /// so the spread landing pass can fence seedlings away from houses and stop
    /// trees from growing up inside them. Built once at construction from the
    /// continuous heightmap — the same sampling the landing pass already uses to
    /// validate terrain — so locally generated and downloaded worlds agree on
    /// where the houses are without shipping them in the base snapshot.
    houses: Vec<Vec<Vec2>>,
    biome_config: BiomeConfig,
    sea_level: f32,
    seed: u32,
    /// Cached totals so `stats()` (called every frame) stays O(1), maintained as
    /// spread adds plants and transitions chunks empty→non-empty.
    population: usize,
    populated_chunks: usize,
    /// Seedlings added by the most recent spread pass, for telemetry.
    last_spread_added: usize,
    /// Cached per-biome fill, refreshed only when saturation changes — `stats()`
    /// reads it every frame, so it must not rescan the world each call.
    biome_fill: Vec<(&'static str, f32, usize)>,
}

fn chunk_index(canon: IVec2) -> usize {
    (canon.y * WORLD_SIZE_CHUNKS + canon.x) as usize
}

impl PlantWorld {
    /// Generate base flora for every canonical chunk in parallel. Terrain and
    /// biome maps are produced transiently per chunk and discarded once the
    /// plants are packed. This is the one-time world-creation cost.
    pub fn generate_base(
        seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
        threads: usize,
        progress: Option<&crate::world_runtime::gen_progress::GenerationProgress>,
    ) -> Self {
        // `Plant` packs the species index into a `u8`; guarantee the herbarium
        // fits before generating millions of plants (this is a long-lived store).
        assert!(
            registry.species.len() <= u8::MAX as usize + 1,
            "PlantWorld packs species_index into a u8, but the herbarium has {} species (max 256)",
            registry.species.len()
        );
        // The saturation skip assumes a plant spreads no further than an adjacent
        // chunk, so seedlings reach at most the 8-neighbourhood.
        let max_spread = registry
            .species
            .iter()
            .map(|s| s.placement.spread_radius)
            .fold(0.0f32, f32::max);
        assert!(
            max_spread < CHUNK_SIZE_METERS,
            "spread radius {max_spread} must stay within one chunk ({CHUNK_SIZE_METERS} m)"
        );

        let n = WORLD_SIZE_CHUNKS;
        let total = (n as usize) * (n as usize);
        let generator =
            ChunkGenerator::new(seed, config, Arc::clone(&registry), Arc::clone(&rivers));

        // Pack one canonical chunk's base flora (and classify its centre biome);
        // terrain/biome maps are transient.
        let centre =
            (CHUNK_GRID_RESOLUTION / 2) * CHUNK_GRID_RESOLUTION + CHUNK_GRID_RESOLUTION / 2;
        let sea_level = config.sea_level;
        let build_one = |idx: usize| -> (Vec<Plant>, Biome, u8) {
            let cx = (idx as i32) % n;
            let cz = (idx as i32) / n;
            let coord = IVec2::new(cx, cz);
            let data = generator.generate_chunk(coord);
            let origin_x = cx as f32 * CHUNK_SIZE_METERS;
            let origin_z = cz as f32 * CHUNK_SIZE_METERS;
            let mut plants = Vec::with_capacity(data.content.base_plants.len());
            for plant in &data.content.base_plants {
                plants.push(Plant::pack(
                    plant,
                    origin_x,
                    origin_z,
                    seed,
                    coord,
                    &data.terrain,
                    sea_level,
                ));
            }
            // Canonical Morton order: makes the base-snapshot position deltas
            // small (the on-disk win) and fixes each plant's index within the
            // chunk, which spread RNG is keyed on — so a downloaded snapshot and a
            // local regeneration produce the same world and evolve identically.
            sort_chunk_canonical(&mut plants);
            plants.shrink_to_fit();
            let centre_height = data.terrain.heights[centre];
            let biome = classify(centre_height, data.terrain.moisture[centre], &config.biome);
            let cell =
                crate::world_runtime::gen_progress::cell_byte(biome, centre_height < sea_level);
            // Surface live progress for the loading visualization: store this
            // chunk's cell color and bump the done counter (single relaxed atomic
            // each — no lock on the generation hot path).
            if let Some(p) = progress {
                p.record(idx, cell);
            }
            (plants, biome, cell)
        };

        #[cfg(not(target_arch = "wasm32"))]
        let built: Vec<(Vec<Plant>, Biome, u8)> = {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads.max(1))
                .build()
                .ok();
            let run = || {
                (0..total)
                    .into_par_iter()
                    .map(build_one)
                    .collect::<Vec<_>>()
            };
            match &pool {
                Some(pool) => pool.install(run),
                None => run(),
            }
        };
        #[cfg(target_arch = "wasm32")]
        let built: Vec<(Vec<Plant>, Biome, u8)> = {
            let _ = threads;
            (0..total).map(build_one).collect()
        };

        let mut chunks = Vec::with_capacity(total);
        let mut biome = Vec::with_capacity(total);
        let mut cell_bytes = Vec::with_capacity(total);
        for (plants, b, cell) in built {
            chunks.push(plants);
            biome.push(b);
            cell_bytes.push(cell);
        }

        let population = chunks.iter().map(Vec::len).sum();
        let populated_chunks = chunks.iter().filter(|c| !c.is_empty()).count();
        let immature = chunks
            .iter()
            .map(|c| c.iter().filter(|p| p.stage < MATURE).count() as u32)
            .collect();
        let base_count = chunks.iter().map(|c| c.len() as u32).collect();
        let saturated = vec![false; chunks.len()];

        // Force a full recompute of per-chunk life timelines on the first
        // growth pass (it also reaps anything whose despawn lies in the past).
        let next_event = vec![0.0; chunks.len()];
        let heightmap = Heightmap::new(seed, config.heightmap.clone());
        // Cap to the same thread budget the flora generation above used.
        let houses = build_house_index(
            seed,
            &config.houses,
            &config.biome,
            &heightmap,
            config.sea_level,
            Some(threads),
        );
        let mut world = Self {
            chunks,
            immature,
            next_event,
            growth_hours: 0.0,
            saturated,
            base_count,
            biome,
            cell_bytes,
            registry,
            heightmap,
            rivers,
            houses,
            biome_config: config.biome.clone(),
            sea_level: config.sea_level,
            seed,
            population,
            populated_chunks,
            last_spread_added: 0,
            biome_fill: Vec::new(),
        };
        world.recompute_biome_fill();
        world
    }

    /// Repaint a [`GenerationProgress`] map from this world's stored cell bytes
    /// and mark every chunk done. Used on the base-snapshot cache path, where the
    /// per-chunk map was loaded rather than recomputed during generation.
    pub fn paint_progress(
        &self,
        progress: &crate::world_runtime::gen_progress::GenerationProgress,
    ) {
        for (idx, &byte) in self.cell_bytes.iter().enumerate() {
            progress.record(idx, byte);
        }
    }

    /// Render a high-resolution, relief-shaded world-map image (RGBA8,
    /// `res`×`res`) by resampling the retained continuous heightmap. Used for the
    /// static in-game map overlay (`M`), which would otherwise reuse the blocky
    /// one-cell-per-chunk loading map.
    pub fn render_world_map(&self, res: usize) -> Vec<u8> {
        crate::world_runtime::world_map::render_world_map(
            &self.heightmap,
            self.sea_level,
            &self.rivers,
            res,
        )
    }

    pub fn sample_height(&self, x: f32, z: f32) -> f32 {
        self.heightmap.sample_height(x, z)
    }

    pub fn inspect_evolution_region(&self, x: f32, z: f32, radius: f32) -> serde_json::Value {
        let radius = radius.max(0.0);
        let radius_sq = radius * radius;
        let world = WORLD_SIZE_METERS as f32;
        let target_x = x.rem_euclid(world);
        let target_z = z.rem_euclid(world);
        let mut count = 0usize;
        let mut gene_sum = [0.0f64; PlantGenes::BYTE_LEN];
        let mut gene_sq = [0.0f64; PlantGenes::BYTE_LEN];
        let mut phenotype_sum = PhenotypeSums::default();
        let mut stage_counts = [0usize; 4];
        let mut species_counts = std::collections::HashMap::<u8, usize>::new();

        for (idx, plants) in self.chunks.iter().enumerate() {
            let cx = (idx as i32) % WORLD_SIZE_CHUNKS;
            let cz = (idx as i32) / WORLD_SIZE_CHUNKS;
            let origin_x = cx as f32 * CHUNK_SIZE_METERS;
            let origin_z = cz as f32 * CHUNK_SIZE_METERS;
            for plant in plants {
                let world_x = origin_x + dequantize_span(plant.local_x);
                let world_z = origin_z + dequantize_span(plant.local_z);
                let dx = torus_delta(world_x, target_x, world);
                let dz = torus_delta(world_z, target_z, world);
                if dx * dx + dz * dz > radius_sq {
                    continue;
                }
                let Some(species) = self.registry.species.get(plant.species as usize) else {
                    continue;
                };
                count += 1;
                let bytes = plant.genes.to_bytes();
                for (i, byte) in bytes.iter().enumerate() {
                    let v = *byte as f64;
                    gene_sum[i] += v;
                    gene_sq[i] += v * v;
                }
                let env = plant_environment(
                    &self.heightmap,
                    &self.rivers,
                    self.sea_level,
                    world_x,
                    world_z,
                );
                phenotype_sum.add(evaluate_phenotype(plant.genes, species, env));
                stage_counts[plant.stage.min(DEAD) as usize] += 1;
                *species_counts.entry(plant.species).or_default() += 1;
            }
        }

        let gene_names = [
            "wet_pref",
            "alt_pref",
            "stress_width",
            "capture",
            "fecundity",
            "seed_mass",
            "dispersal",
            "timing",
        ];
        let mut genes = serde_json::Map::new();
        for (i, name) in gene_names.iter().enumerate() {
            let mean = if count == 0 {
                0.0
            } else {
                gene_sum[i] / count as f64
            };
            let variance = if count == 0 {
                0.0
            } else {
                (gene_sq[i] / count as f64 - mean * mean).max(0.0)
            };
            genes.insert(
                (*name).to_string(),
                serde_json::json!({ "mean": mean, "stddev": variance.sqrt() }),
            );
        }

        let mut top_species: Vec<_> = species_counts.into_iter().collect();
        top_species.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
        let top_species: Vec<_> = top_species
            .into_iter()
            .take(8)
            .map(|(idx, count)| {
                let name = self
                    .registry
                    .species
                    .get(idx as usize)
                    .map(|s| s.name.as_str())
                    .unwrap_or("unknown");
                serde_json::json!({ "species": idx, "name": name, "count": count })
            })
            .collect();

        serde_json::json!({
            "center": { "x": x, "z": z, "radius": radius },
            "plant_count": count,
            "genes": genes,
            "phenotype": phenotype_sum.mean_json(count),
            "mean_abiotic_fitness": phenotype_sum.mean_abiotic_fitness(count),
            "mean_competition_stress": phenotype_sum.mean_stress(count),
            "stages": {
                "seedling": stage_counts[stage_to_u8(GrowthStage::Seedling) as usize],
                "young": stage_counts[stage_to_u8(GrowthStage::Young) as usize],
                "mature": stage_counts[MATURE as usize],
                "dead": stage_counts[DEAD as usize],
            },
            "top_species": top_species,
        })
    }

    /// Total plants across the whole world (loaded or not). O(1).
    pub fn population(&self) -> usize {
        self.population
    }

    /// Number of canonical chunks holding at least one plant. O(1).
    pub fn populated_chunks(&self) -> usize {
        self.populated_chunks
    }

    /// Reconstruct renderable instances for the raw chunk at `raw_coord`, reading
    /// the canonical chunk's plant list and resampling ground height from
    /// `terrain`. Dead snags carry their decay fraction (0 freshly dead → 1
    /// about to despawn) so the renderer can ease them smaller as they rot.
    pub fn instances_for(
        &self,
        raw_coord: IVec2,
        terrain: &ChunkTerrain,
        overlay: EvolutionOverlayMode,
    ) -> Vec<PlantInstance> {
        let canon = canonical_chunk(raw_coord);
        let idx = chunk_index(canon);
        let base_count = self.base_count[idx] as usize;
        self.chunks[idx]
            .iter()
            .enumerate()
            .map(|(pi, plant)| {
                let mut instance = plant.to_instance(raw_coord, terrain);
                if let Some(species) = self.registry.species.get(plant.species as usize) {
                    let env = plant_environment(
                        &self.heightmap,
                        &self.rivers,
                        self.sea_level,
                        instance.position.x,
                        instance.position.z,
                    );
                    let phenotype = evaluate_phenotype(plant.genes, species, env);
                    instance.height_scale = phenotype.height_scale;
                    instance.width_scale = phenotype.width_scale;
                    instance.stress = phenotype.stress;
                    instance.leaf_tint = overlay_tint(overlay, plant, phenotype);
                    if plant.stage == DEAD {
                        let schedule = life_schedule(
                            self.seed,
                            idx,
                            plant,
                            species,
                            pi < base_count,
                            plant_life_environment(
                                &self.heightmap,
                                &self.rivers,
                                self.sea_level,
                                idx,
                                plant,
                            ),
                        );
                        if schedule.gone_at.is_finite() {
                            let span = (schedule.gone_at - schedule.dead_at).max(1e-6);
                            let age = (self.growth_hours - plant.born_hour as f64).max(0.0);
                            instance.decay =
                                ((age - schedule.dead_at) / span).clamp(0.0, 1.0) as f32;
                        }
                    }
                }
                instance
            })
            .collect()
    }

    /// Advance the life cycle on the global clock. Stages — including death and
    /// despawn — are analytic functions of each plant's `born_hour` and stable
    /// identity, so this only walks chunks whose earliest pending transition
    /// (`next_event`) has arrived. Plants past their `gone_at` are removed,
    /// freeing their ground: the chunk (and its 8 neighbours, which can reach
    /// the freed spot) is un-saturated so the spread pass resumes there.
    /// Returns whether anything changed (so the render bridge can refresh
    /// loaded chunks).
    pub fn tick_growth(&mut self, total_hours: f64) -> bool {
        self.growth_hours = total_hours;
        let registry = Arc::clone(&self.registry);
        let seed = self.seed;
        let heightmap = &self.heightmap;
        let rivers = &self.rivers;
        let sea_level = self.sea_level;
        let mut changed = false;
        let mut reaped_chunks: Vec<usize> = Vec::new();
        for idx in 0..self.chunks.len() {
            if total_hours < self.next_event[idx] {
                continue;
            }
            let base_count = self.base_count[idx] as usize;
            let chunk = &mut self.chunks[idx];
            if chunk.is_empty() {
                self.next_event[idx] = f64::INFINITY;
                continue;
            }
            let mut immature = 0u32;
            let mut next_event = f64::INFINITY;
            let mut removed_base = 0u32;
            let mut write = 0usize;
            for read in 0..chunk.len() {
                let mut plant = chunk[read];
                let Some(species) = registry.species.get(plant.species as usize) else {
                    chunk[write] = plant;
                    write += 1;
                    continue;
                };
                let env = plant_life_environment(heightmap, rivers, sea_level, idx, &plant);
                let schedule = life_schedule(seed, idx, &plant, species, read < base_count, env);
                let age = (total_hours - plant.born_hour as f64).max(0.0);
                let Some(stage) = schedule.stage_at(age) else {
                    // Despawned: drop the record, freeing its spacing-grid spot
                    // for the next spread pass (the grid is rebuilt per pass).
                    if read < base_count {
                        removed_base += 1;
                    }
                    changed = true;
                    continue;
                };
                if stage != plant.stage {
                    plant.stage = stage;
                    changed = true;
                }
                if plant.stage < MATURE {
                    immature += 1;
                }
                next_event = next_event.min(plant.born_hour as f64 + schedule.next_threshold(age));
                chunk[write] = plant;
                write += 1;
            }
            let removed = chunk.len() - write;
            chunk.truncate(write);
            self.immature[idx] = immature;
            self.next_event[idx] = next_event;
            if removed > 0 {
                self.base_count[idx] -= removed_base;
                self.population -= removed;
                if self.chunks[idx].is_empty() {
                    self.populated_chunks -= 1;
                }
                reaped_chunks.push(idx);
            }
        }
        if !reaped_chunks.is_empty() {
            // Freed ground is reachable by the 8-neighbourhood's spread radius,
            // so the whole block must wake up or the gap never refills.
            let mut any_unsaturated = false;
            for &idx in &reaped_chunks {
                any_unsaturated |= self.unsaturate_neighbourhood(idx);
            }
            if any_unsaturated {
                self.recompute_biome_fill();
            }
        }
        changed
    }

    /// Clear the saturation flag on chunk `idx` and its 8 (wrapped) neighbours.
    /// Returns whether any flag actually flipped.
    fn unsaturate_neighbourhood(&mut self, idx: usize) -> bool {
        let n = WORLD_SIZE_CHUNKS;
        let cx = (idx as i32) % n;
        let cz = (idx as i32) / n;
        let mut flipped = false;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let nx = (cx + dx).rem_euclid(n);
                let nz = (cz + dz).rem_euclid(n);
                let nidx = (nz * n + nx) as usize;
                // Unit-test worlds are smaller than the real canonical grid;
                // a wrapped neighbour outside them has no flag to clear.
                if let Some(flag) = self.saturated.get_mut(nidx) {
                    flipped |= std::mem::replace(flag, false);
                }
            }
        }
        flipped
    }

    /// Global spread pass on the canonical grid. Two-phase to stay race-free:
    /// phase 1 (parallel, read-only) has every mature plant emit seedling
    /// candidates targeting a canonical chunk; phase 2 validates each candidate
    /// against the target chunk's terrain (point-sampled) and existing plants
    /// (spacing) and appends the survivors. Spacing is the capacity gate, so the
    /// population grows toward a bounded full state and then stops. Returns
    /// whether any seedling was added.
    pub fn tick_spread(&mut self, born_hour: f64) -> bool {
        let total = self.chunks.len();

        // Per-sim-day reproduction bucket. The spread roll and seedling positions
        // are salted with it so each day's pass is an *independent* reproduction
        // event rather than the same deterministic roll repeated forever. Without
        // this, a plant that fails its one roll never reproduces, and the catch-up
        // passes the runtime fires at high `day_speed` would be exact duplicates —
        // births would never keep pace with the analytic death rate.
        let bucket = day_bucket(born_hour);
        let emit_ctx = EmitContext {
            registry: &self.registry,
            heightmap: &self.heightmap,
            rivers: &self.rivers,
            sea_level: self.sea_level,
            seed: self.seed,
            bucket,
        };

        // Phase 1: emit candidates. Read-only over the whole grid. A chunk is
        // skipped only when it and all eight neighbours are saturated, so a packed
        // chunk still seeds a not-yet-full neighbour across the border.
        let saturated = &self.saturated;
        let emit = |idx: usize| {
            if neighbourhood_saturated(saturated, idx) {
                return Vec::new();
            }
            emit_chunk_candidates(idx, &self.chunks, &emit_ctx)
        };
        #[cfg(not(target_arch = "wasm32"))]
        let candidates: Vec<SpreadCandidate> = (0..total).into_par_iter().flat_map(emit).collect();
        #[cfg(target_arch = "wasm32")]
        let candidates: Vec<SpreadCandidate> = (0..total).flat_map(emit).collect();

        if candidates.is_empty() {
            return false;
        }

        // Scatter candidates to their target chunk.
        let mut incoming: Vec<Vec<SpreadCandidate>> = (0..total).map(|_| Vec::new()).collect();
        for candidate in candidates {
            incoming[candidate.target as usize].push(candidate);
        }

        // Phase 2: validate + append, one chunk per task (chunk Vecs are disjoint).
        let ctx = SpreadContext {
            seed: self.seed,
            heightmap: &self.heightmap,
            rivers: &self.rivers,
            registry: &self.registry,
            biome_config: &self.biome_config,
            houses: &self.houses,
            sea_level: self.sea_level,
            born_hour: born_hour as f32,
        };
        let chunks = &mut self.chunks;
        let landed = |(plants, cands): (&mut Vec<Plant>, &mut Vec<SpreadCandidate>)| {
            land_chunk_candidates(plants, cands, &ctx)
        };
        #[cfg(not(target_arch = "wasm32"))]
        let accepted: Vec<u32> = chunks
            .par_iter_mut()
            .zip(incoming.par_iter_mut())
            .map(landed)
            .collect();
        #[cfg(target_arch = "wasm32")]
        let accepted: Vec<u32> = chunks
            .iter_mut()
            .zip(incoming.iter_mut())
            .map(landed)
            .collect();

        // Serial fold of the per-chunk results into the cached counters and the
        // saturation flags.
        let mut added = 0usize;
        for (idx, &count) in accepted.iter().enumerate() {
            if count > 0 {
                added += count as usize;
                // It gained plants, so it is not saturated and may spread once
                // those seedlings mature.
                self.saturated[idx] = false;
                // `len() == count` means the chunk held nothing before this pass,
                // so it just transitioned empty → non-empty.
                if self.chunks[idx].len() == count as usize {
                    self.populated_chunks += 1;
                }
                self.immature[idx] += count;
                self.population += count as usize;
                // New seedlings bring new life thresholds; recompute the
                // chunk's timeline on the next growth pass.
                self.next_event[idx] = 0.0;
            } else {
                // Nothing landed: saturated iff nothing is left growing here. A
                // neighbour maturing and spreading in (above) clears this again.
                self.saturated[idx] = self.immature[idx] == 0;
            }
        }
        self.last_spread_added = added;
        // Saturation may have flipped this pass; refresh the cached telemetry.
        self.recompute_biome_fill();
        added > 0
    }

    /// Seedlings added by the most recent spread pass.
    pub fn last_spread_added(&self) -> usize {
        self.last_spread_added
    }

    /// Approximate resident bytes of the store (packed plants + per-chunk Vec
    /// headers + the parallel index Vecs + the house index).
    pub fn resident_bytes(&self) -> usize {
        let plant = self.population * std::mem::size_of::<Plant>();
        let headers = self.chunks.len() * std::mem::size_of::<Vec<Plant>>();
        let side = self.chunks.len();
        // House index: outer per-chunk Vec headers + each inner Vec's allocated XZ.
        let house_headers = self.houses.len() * std::mem::size_of::<Vec<Vec2>>();
        let house_positions: usize = self
            .houses
            .iter()
            .map(|h| h.capacity() * std::mem::size_of::<Vec2>())
            .sum();
        let indices = side
            * (std::mem::size_of::<u32>() // immature
                + std::mem::size_of::<bool>() // saturated
                + std::mem::size_of::<u32>() // base_count
                + std::mem::size_of::<Biome>()); // biome
        plant + headers + indices + house_headers + house_positions
    }

    /// Per-biome fill: the percentage (0–100) of each biome's chunks that are
    /// saturated (done spreading), as `(biome_name, percent, chunk_count)` for the
    /// five biomes; biomes with no chunks report 0. O(1): it returns a cache
    /// refreshed only when saturation can change (spread passes, load, creation),
    /// since `stats()` reads it every frame.
    pub fn biome_fill_percents(&self) -> Vec<(&'static str, f32, usize)> {
        self.biome_fill.clone()
    }

    /// Recompute the per-biome fill cache in one pass. Called after generation,
    /// load, and any spread pass that can flip saturation.
    fn recompute_biome_fill(&mut self) {
        let mut total = [0usize; BIOME_SLOTS];
        let mut saturated = [0usize; BIOME_SLOTS];
        for (idx, &b) in self.biome.iter().enumerate() {
            let slot = biome_slot(b);
            total[slot] += 1;
            if self.saturated[idx] {
                saturated[slot] += 1;
            }
        }
        self.biome_fill = BIOME_ORDER
            .iter()
            .map(|&biome| {
                let slot = biome_slot(biome);
                let percent = if total[slot] == 0 {
                    0.0
                } else {
                    saturated[slot] as f32 / total[slot] as f32 * 100.0
                };
                (biome_name(biome), percent, total[slot])
            })
            .collect();
    }

    /// Save the spread suffix of every chunk (the plants beyond the regenerable
    /// base) as a compact binary blob. Round-trips the full world together with
    /// the seed-deterministic base.
    pub fn save_spread(
        &self,
        storage: &dyn crate::world_core::storage::Storage,
    ) -> anyhow::Result<()> {
        let spread_chunks: Vec<usize> = (0..self.chunks.len())
            .filter(|&i| self.chunks[i].len() as u32 > self.base_count[i])
            .collect();

        // header(16) + per-chunk(8) + PLANT_BYTES per spread plant.
        let plant_total: usize = spread_chunks
            .iter()
            .map(|&i| self.chunks[i].len() - self.base_count[i] as usize)
            .sum();
        let mut buf = Vec::with_capacity(16 + spread_chunks.len() * 8 + plant_total * PLANT_BYTES);
        buf.extend_from_slice(&SPREAD_MAGIC.to_le_bytes());
        buf.extend_from_slice(&SPREAD_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.seed.to_le_bytes());
        buf.extend_from_slice(&(spread_chunks.len() as u32).to_le_bytes());
        for &i in &spread_chunks {
            let suffix = &self.chunks[i][self.base_count[i] as usize..];
            buf.extend_from_slice(&(i as u32).to_le_bytes());
            buf.extend_from_slice(&(suffix.len() as u32).to_le_bytes());
            for plant in suffix {
                write_plant(&mut buf, plant);
            }
        }
        storage.save_bytes("plants", &buf)
    }

    /// Re-apply a previously saved spread suffix on top of the freshly generated
    /// base. Ignored (with a warning) if absent, malformed, or for another seed.
    /// Returns how many plants were restored.
    pub fn apply_saved_spread(
        &mut self,
        storage: &dyn crate::world_core::storage::Storage,
    ) -> usize {
        self.apply_saved_spread_bytes(storage.load_bytes("plants").as_deref())
    }

    /// As [`apply_saved_spread`], but from an already-loaded blob. The async
    /// generation path reads the save bytes on the main thread (storage handles
    /// are not `Send`) and hands them to the worker through this.
    pub fn apply_saved_spread_bytes(&mut self, bytes: Option<&[u8]>) -> usize {
        let Some(buf) = bytes else {
            return 0;
        };
        match self.read_spread(buf) {
            Ok(added) => added,
            Err(err) => {
                log::warn!("ignoring saved spread state: {err}");
                0
            }
        }
    }

    fn read_spread(&mut self, buf: &[u8]) -> anyhow::Result<usize> {
        let mut cur = Cursor::new(buf);
        if cur.read_u32()? != SPREAD_MAGIC {
            anyhow::bail!("bad magic");
        }
        if cur.read_u32()? != SPREAD_VERSION {
            anyhow::bail!("unsupported version");
        }
        let seed = cur.read_u32()?;
        if seed != self.seed {
            anyhow::bail!("seed {seed} does not match world seed {}", self.seed);
        }
        let chunk_count = cur.read_u32()? as usize;
        let mut added = 0usize;
        for _ in 0..chunk_count {
            let idx = cur.read_u32()? as usize;
            let count = cur.read_u32()? as usize;
            if idx >= self.chunks.len() {
                anyhow::bail!("chunk index {idx} out of range");
            }
            // Bound `count` by the bytes actually left before reserving, so a
            // corrupted length can't trigger a huge allocation ahead of the EOF.
            if count.saturating_mul(PLANT_BYTES) > cur.remaining() {
                anyhow::bail!("declared {count} plants exceed the remaining data");
            }
            // The base list was shrunk to fit during generation; reserve the
            // suffix up front so restoring it is one growth, not many.
            self.chunks[idx].reserve(count);
            for _ in 0..count {
                let plant = read_plant(&mut cur)?;
                if plant.stage < MATURE {
                    self.immature[idx] += 1;
                }
                self.chunks[idx].push(plant);
                added += 1;
            }
        }
        // Recompute cached totals from scratch — cheap next to world generation.
        self.population = self.chunks.iter().map(Vec::len).sum();
        self.populated_chunks = self.chunks.iter().filter(|c| !c.is_empty()).count();
        // Seed saturation so a resumed, settled world is cheap on its first spread
        // pass instead of emitting for every chunk: an all-mature chunk is treated
        // as saturated (a chunk that still has room re-clears the flag the moment a
        // neighbour's seedling lands in it).
        for (idx, &immature) in self.immature.iter().enumerate() {
            self.saturated[idx] = immature == 0;
        }
        self.recompute_biome_fill();
        Ok(added)
    }

    /// Brotli quality for the snapshot shipped on GitHub and downloaded by
    /// clients: a high level (~31 MiB, near the practical size floor — q11 saves
    /// only ~1 MiB for ~20 s more) at ~1 min to encode, paid once in CI and
    /// never on a player's machine. See [`serialize_base`](Self::serialize_base).
    pub const DOWNLOAD_QUALITY: u32 = 10;
    /// Brotli quality for a locally generated cache: ~30% smaller than raw
    /// (~67→47 MiB) in well under a second, so a New Game pays no perceptible
    /// compression cost. Higher levels hit sharp diminishing returns — matching
    /// the download's size would cost the full ~1 min encode for ~16 MiB more.
    pub const LOCAL_QUALITY: u32 = 1;

    /// Serialize the whole base world — every canonical chunk's full plant list
    /// plus its biome and overview-map cell — as a compact binary blob, tagged
    /// with `gen_key` (a hash of the generation inputs) and the seed. This is the
    /// expensive [`generate_base`] output cached so a later New Game can skip
    /// regeneration. Distinct from [`save_spread`], which persists only the
    /// per-game spread delta on top of a regenerable base.
    ///
    /// Layout (v3): a small plaintext header, then three independently
    /// Brotli-compressed columnar sections. Splitting by field lets the
    /// compressor exploit each column's statistics — `meta` (biome/cell/count)
    /// and `attr` (height/rotation/species/genes) are low-entropy and collapse, while
    /// `pos` (the irreducible position core) is kept small by Morton-sorting each
    /// chunk and storing varint deltas of the sorted codes. Plants are assumed to
    /// already be in canonical Morton order (generation and load both sort), so
    /// deltas are non-negative.
    ///
    /// `quality` is the Brotli level (0–11) for each section. The prebuilt
    /// snapshot shipped on GitHub uses [`Self::DOWNLOAD_QUALITY`] (q10) so the
    /// once-in-CI encode buys the smallest practical download; a locally
    /// generated cache uses [`Self::LOCAL_QUALITY`] (q1), which is ~30% smaller
    /// than raw for a fraction of a second — q10 would add ~minutes to every New
    /// Game for a file that never leaves the disk. Brotli decode is
    /// quality-agnostic, so every quality yields the same v2 layout (only the
    /// compressed bytes differ) and loads through the identical path.
    pub fn serialize_base(&self, gen_key: u64, quality: u32) -> Vec<u8> {
        let total = self.chunks.len();

        let mut meta = Vec::with_capacity(total * 6);
        let mut pos = Vec::new();
        let mut attr = Vec::new();
        for idx in 0..total {
            let plants = &self.chunks[idx];
            meta.push(biome_slot(self.biome[idx]) as u8);
            meta.push(self.cell_bytes[idx]);
            meta.extend_from_slice(&(plants.len() as u32).to_le_bytes());
            let mut prev = 0u32;
            for p in plants {
                let code = morton10(p.local_x >> POS_SHIFT, p.local_z >> POS_SHIFT);
                // Canonical Morton order is a hard precondition of the delta
                // encoding, so fail fast in every build rather than wrapping into
                // a corrupt position stream that would only surface at load.
                assert!(code >= prev, "base chunk not in canonical Morton order");
                write_varint(&mut pos, code - prev);
                prev = code;
                attr.push(pack_height(dequantize_span(p.height)));
                attr.push((p.rotation >> 8) as u8);
                attr.push(p.species);
                attr.extend_from_slice(&p.genes.to_bytes());
            }
        }

        let mut buf = Vec::new();
        buf.extend_from_slice(&BASE_MAGIC.to_le_bytes());
        buf.extend_from_slice(&BASE_VERSION.to_le_bytes());
        buf.extend_from_slice(&gen_key.to_le_bytes());
        buf.extend_from_slice(&self.seed.to_le_bytes());
        buf.extend_from_slice(&(WORLD_SIZE_CHUNKS as u32).to_le_bytes());
        buf.push(POS_BITS as u8);
        buf.extend_from_slice(&[0u8; 3]); // reserved (keeps the header word-aligned)
        write_section(&mut buf, &meta, quality);
        write_section(&mut buf, &pos, quality);
        write_section(&mut buf, &attr, quality);
        buf
    }

    /// Reconstruct a base world from a [`serialize_base`] blob, rebuilding the
    /// derived indices (immature counts, base counts, totals, biome fill). The
    /// snapshot is accepted only when its magic, version, generation key, seed,
    /// and world size all match the current world; on any mismatch or corruption
    /// it returns `None` (with a warning) so the caller falls back to generation.
    pub fn from_base_snapshot(
        bytes: &[u8],
        gen_key: u64,
        expected_seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
    ) -> Option<Self> {
        match Self::read_base(bytes, gen_key, expected_seed, config, registry, rivers) {
            Ok(world) => Some(world),
            Err(err) => {
                log::warn!("ignoring cached base world: {err}");
                None
            }
        }
    }

    fn read_base(
        buf: &[u8],
        gen_key: u64,
        expected_seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
    ) -> anyhow::Result<Self> {
        let mut cur = Cursor::new(buf);
        if cur.read_u32()? != BASE_MAGIC {
            anyhow::bail!("bad magic");
        }
        if cur.read_u32()? != BASE_VERSION {
            anyhow::bail!("unsupported version");
        }
        let key = cur.read_u64()?;
        if key != gen_key {
            anyhow::bail!("generation key {key:#018x} does not match {gen_key:#018x}");
        }
        let seed = cur.read_u32()?;
        if seed != expected_seed {
            anyhow::bail!("seed {seed} does not match world seed {expected_seed}");
        }
        let n = cur.read_u32()?;
        if n != WORLD_SIZE_CHUNKS as u32 {
            anyhow::bail!("world size {n} does not match {WORLD_SIZE_CHUNKS}");
        }
        let pos_bits = cur.read_u8()?;
        if pos_bits as u32 != POS_BITS {
            anyhow::bail!("position precision {pos_bits} does not match {POS_BITS}");
        }
        cur.skip(3)?; // reserved
        let total = (n as usize) * (n as usize);

        // Three Brotli sections, decompressed into their own buffers and walked
        // in lockstep: `meta` drives the per-chunk loop, `pos`/`attr` feed each
        // plant. Section lengths are bounded inside `read_section`. Decode is
        // independent of the quality each was written at.
        let meta = read_section(&mut cur)?;
        let pos = read_section(&mut cur)?;
        let attr = read_section(&mut cur)?;
        let mut meta_cur = Cursor::new(&meta);
        let mut pos_cur = Cursor::new(&pos);
        let mut attr_cur = Cursor::new(&attr);

        let mut chunks: Vec<Vec<Plant>> = Vec::with_capacity(total);
        let mut biome = Vec::with_capacity(total);
        let mut cell_bytes = Vec::with_capacity(total);
        for _ in 0..total {
            let (plants, b, cell) = parse_base_chunk(&mut meta_cur, &mut pos_cur, &mut attr_cur)?;
            chunks.push(plants);
            biome.push(b);
            cell_bytes.push(cell);
        }

        Ok(Self::assemble_base(
            chunks, biome, cell_bytes, seed, config, registry, rivers,
        ))
    }

    /// Assemble a world from the per-chunk lists decoded from a base snapshot,
    /// rebuilding the derived indices (immature/base counts, totals, biome fill).
    /// Shared by the one-shot [`read_base`] and the incremental [`BaseLoader`] so
    /// both produce an identical world.
    fn assemble_base(
        chunks: Vec<Vec<Plant>>,
        biome: Vec<Biome>,
        cell_bytes: Vec<u8>,
        seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
    ) -> Self {
        let population = chunks.iter().map(Vec::len).sum();
        let populated_chunks = chunks.iter().filter(|c| !c.is_empty()).count();
        let immature = chunks
            .iter()
            .map(|c| c.iter().filter(|p| p.stage < MATURE).count() as u32)
            .collect();
        let base_count = chunks.iter().map(|c| c.len() as u32).collect();
        let saturated = vec![false; chunks.len()];

        // Force a full recompute of per-chunk life timelines on the first
        // growth pass (it also reaps anything whose despawn lies in the past).
        let next_event = vec![0.0; chunks.len()];
        let heightmap = Heightmap::new(seed, config.heightmap.clone());
        // Snapshot load is light and carries no thread budget; use the ambient pool.
        let houses = build_house_index(
            seed,
            &config.houses,
            &config.biome,
            &heightmap,
            config.sea_level,
            None,
        );
        let mut world = Self {
            chunks,
            immature,
            next_event,
            growth_hours: 0.0,
            saturated,
            base_count,
            biome,
            cell_bytes,
            registry,
            heightmap,
            rivers,
            houses,
            biome_config: config.biome.clone(),
            sea_level: config.sea_level,
            seed,
            population,
            populated_chunks,
            last_spread_added: 0,
            biome_fill: Vec::new(),
        };
        world.recompute_biome_fill();
        world
    }
}

/// Decode one chunk's record from the three section cursors, walked in lockstep:
/// `meta` carries biome/cell/count, `pos` the Morton-delta positions, `attr` the
/// packed height/rotation/species/genes. Shared by the one-shot
/// [`PlantWorld::read_base`] and the incremental [`BaseLoader`] so they
/// reconstruct plants identically.
fn parse_base_chunk(
    meta_cur: &mut Cursor<'_>,
    pos_cur: &mut Cursor<'_>,
    attr_cur: &mut Cursor<'_>,
) -> anyhow::Result<(Vec<Plant>, Biome, u8)> {
    let biome_code = meta_cur.read_u8()?;
    let cell = meta_cur.read_u8()?;
    let count = meta_cur.read_u32()? as usize;
    // Each plant consumes at least one varint byte from `pos` and exactly eleven
    // from `attr`; bound `count` by both before reserving so a corrupted length
    // can't trigger a huge allocation ahead of EOF. Also cap by the number of
    // distinct Morton cells — a 256 m chunk on the 10-bit grid can't hold more,
    // and base plants are metres apart, so this never rejects real data but stops
    // a crafted count from driving a multi-gigabyte `with_capacity`.
    if count > MORTON_CELLS
        || count > pos_cur.remaining()
        || count.saturating_mul(BASE_ATTR_BYTES) > attr_cur.remaining()
    {
        anyhow::bail!("declared {count} plants exceed the section data");
    }
    let mut plants = Vec::with_capacity(count);
    let mut prev = 0u32;
    for _ in 0..count {
        let delta = read_varint(pos_cur)?;
        let code = prev
            .checked_add(delta)
            .ok_or_else(|| anyhow::anyhow!("position code overflow"))?;
        prev = code;
        // `unmorton10` only reads the low `2*POS_BITS` bits, so reject any
        // out-of-range code rather than silently wrapping it to a bogus
        // in-range position.
        if code as usize >= MORTON_CELLS {
            anyhow::bail!("position code {code} out of range");
        }
        let (x10, z10) = unmorton10(code);
        let h8 = attr_cur.read_u8()?;
        let r8 = attr_cur.read_u8()?;
        let species = attr_cur.read_u8()?;
        let genes = PlantGenes::from_bytes(attr_cur.read_array()?);
        plants.push(Plant {
            local_x: x10 << POS_SHIFT,
            local_z: z10 << POS_SHIFT,
            height: unpack_height(h8),
            rotation: (r8 as u16) << 8,
            species,
            stage: MATURE,
            born_hour: 0.0,
            genes,
        });
    }
    Ok((plants, biome_from_slot(biome_code), cell))
}

/// True when chunk `idx` and all eight of its (wrapped) neighbours are saturated,
/// so the chunk's plants cannot place a seedling anywhere this pass.
fn neighbourhood_saturated(saturated: &[bool], idx: usize) -> bool {
    if !saturated[idx] {
        return false;
    }
    let n = WORLD_SIZE_CHUNKS;
    let cx = (idx as i32) % n;
    let cz = (idx as i32) / n;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let nx = (cx + dx).rem_euclid(n);
            let nz = (cz + dz).rem_euclid(n);
            if !saturated[(nz * n + nx) as usize] {
                return false;
            }
        }
    }
    true
}

/// A seedling proposed by phase 1, targeting canonical chunk `target`.
struct SpreadCandidate {
    target: u32,
    /// Position local to the target chunk, in metres `[0, CHUNK_SIZE_METERS)`.
    local_x: f32,
    local_z: f32,
    /// Canonical world position, for point-sampling terrain.
    world_x: f32,
    world_z: f32,
    height: f32,
    rotation: f32,
    species: u8,
    genes: PlantGenes,
    /// Deterministic ordering key (source chunk, plant, seed) so landing order
    /// — and thus which candidates win spacing — is independent of parallelism.
    order: u64,
}

struct SpreadContext<'a> {
    seed: u32,
    heightmap: &'a Heightmap,
    rivers: &'a RiverField,
    registry: &'a PlantRegistry,
    biome_config: &'a BiomeConfig,
    /// House XZ positions per canonical chunk, for the keep-clear-of-houses guard.
    houses: &'a [Vec<Vec2>],
    sea_level: f32,
    born_hour: f32,
}

struct EmitContext<'a> {
    registry: &'a PlantRegistry,
    heightmap: &'a Heightmap,
    rivers: &'a RiverField,
    sea_level: f32,
    seed: u32,
    bucket: u32,
}

fn emit_chunk_candidates(
    idx: usize,
    chunks: &[Vec<Plant>],
    ctx: &EmitContext<'_>,
) -> Vec<SpreadCandidate> {
    let n = WORLD_SIZE_CHUNKS;
    let world = WORLD_SIZE_METERS as f32;
    let cx = (idx as i32) % n;
    let cz = (idx as i32) / n;
    let canon = IVec2::new(cx, cz);

    let mut out = Vec::new();
    for (pi, plant) in chunks[idx].iter().enumerate() {
        if plant.stage != MATURE {
            continue;
        }
        let Some(species) = ctx.registry.species.get(plant.species as usize) else {
            continue;
        };
        let src_x = cx as f32 * CHUNK_SIZE_METERS + dequantize_span(plant.local_x);
        let src_z = cz as f32 * CHUNK_SIZE_METERS + dequantize_span(plant.local_z);
        let parent_env = plant_environment(ctx.heightmap, ctx.rivers, ctx.sea_level, src_x, src_z);
        let parent_pheno = evaluate_phenotype(plant.genes, species, parent_env);
        let fecundity = PlantGenes::unit(plant.genes.fecundity);
        let spread_chance = (species.placement.spread_chance
            * (0.35 + parent_pheno.abiotic_fitness * 0.65)
            * (0.65 + fecundity * 0.7)
            * (1.0 - parent_pheno.stress * 0.35))
            .clamp(0.0, 1.0);
        if spread_roll(ctx.seed, canon, pi as u32, ctx.bucket) >= spread_chance {
            continue;
        }

        let [h_lo, h_hi] = species.height_range;
        let count = ((spread_seed_count(ctx.seed, canon, pi as u32, ctx.bucket) as f32
            * parent_pheno.seed_count_scale)
            .round() as u32)
            .clamp(1, 4);
        let spread_radius =
            species.placement.spread_radius.max(0.0) * parent_pheno.spread_radius_scale.max(0.0);
        for seed_i in 0..count {
            // Salt the per-seedling jitter with the day bucket too, so a fresh
            // roll also aims at a fresh spot — otherwise each day's survivor would
            // re-target the same (already occupied) cells and never fill new gaps.
            let sub = bucket_salt(
                (pi as u32).wrapping_mul(31).wrapping_add(seed_i),
                ctx.bucket,
            );
            let angle = hash_unit(ctx.seed.wrapping_add(4101), canon, sub) * std::f32::consts::TAU;
            let distance =
                hash_unit(ctx.seed.wrapping_add(4102), canon, sub).sqrt() * spread_radius;
            let height = h_lo + hash_unit(ctx.seed.wrapping_add(4201), canon, sub) * (h_hi - h_lo);
            let rotation =
                hash_unit(ctx.seed.wrapping_add(4202), canon, sub) * std::f32::consts::TAU;
            let genes = inherit_and_mutate(
                plant.genes,
                ctx.seed,
                MutationKey {
                    chunk: canon,
                    parent_index: pi as u32,
                    bucket: ctx.bucket,
                    seed_index: seed_i,
                },
            );

            // Wrap the seedling onto the torus, then split into canonical chunk +
            // local offset.
            let world_x = (src_x + angle.cos() * distance).rem_euclid(world);
            let world_z = (src_z + angle.sin() * distance).rem_euclid(world);
            let tcx = ((world_x / CHUNK_SIZE_METERS).floor() as i32).clamp(0, n - 1);
            let tcz = ((world_z / CHUNK_SIZE_METERS).floor() as i32).clamp(0, n - 1);
            out.push(SpreadCandidate {
                target: (tcz * n + tcx) as u32,
                local_x: world_x - tcx as f32 * CHUNK_SIZE_METERS,
                local_z: world_z - tcz as f32 * CHUNK_SIZE_METERS,
                world_x,
                world_z,
                height,
                rotation,
                species: plant.species,
                genes,
                order: ((idx as u64) << 24) | ((pi as u64) << 4) | seed_i as u64,
            });
        }
    }
    out
}

fn land_chunk_candidates(
    plants: &mut Vec<Plant>,
    candidates: &mut [SpreadCandidate],
    ctx: &SpreadContext<'_>,
) -> u32 {
    if candidates.is_empty() {
        return 0;
    }
    candidates.sort_by_key(|c| c.order);

    // Spatial grid over the chunk's existing plants so each spacing query touches
    // only a 3×3 cell neighbourhood instead of every plant — without it the pass
    // is O(candidates × plants) and chokes as chunks densify.
    let mut grid = SpacingGrid::build(plants);
    let mut competition = CompetitionGrid::build(plants);

    let mut accepted = 0u32;
    for candidate in candidates.iter() {
        let Some(species) = ctx.registry.species.get(candidate.species as usize) else {
            continue;
        };
        let placement = &species.placement;

        // Cheap checks first; `slope_at` (12 noise samples) is left for last so a
        // candidate rejected by altitude/moisture/biome/spacing never pays for it.
        let height = ctx
            .heightmap
            .sample_height(candidate.world_x, candidate.world_z);
        if height < ctx.sea_level
            || height < placement.min_altitude
            || height > placement.max_altitude
        {
            continue;
        }
        // Reject seedlings that land in a river channel — mirrors the base-flora
        // guard so spread can't slowly recolonise the water the base pass kept clear.
        if ctx.rivers.wetness(candidate.world_x, candidate.world_z) > MAX_PLANTABLE_WETNESS {
            continue;
        }
        let moisture = ctx
            .heightmap
            .sample_moisture(candidate.world_x, candidate.world_z);
        if moisture < placement.min_moisture || moisture > placement.max_moisture {
            continue;
        }
        let biome = classify(height, moisture, ctx.biome_config);
        if !placement.biomes.iter().any(|b| b == biome_name(biome)) {
            continue;
        }

        // Spacing against everything already in the chunk (base + earlier
        // survivors this pass). Horizontal distance — y is terrain-derived.
        let spacing = min_spacing(&species.kind);
        if grid.any_within(candidate.local_x, candidate.local_z, spacing) {
            continue;
        }

        // Keep seedlings clear of houses so reproduction can't slowly fill the
        // buildings with trees. Cheap (house lists are sparse) so it precedes the
        // 12-sample slope test.
        if near_house(
            ctx.houses,
            candidate.world_x,
            candidate.world_z,
            house_clearance(&species.kind),
        ) {
            continue;
        }

        if slope_at(ctx.heightmap, candidate.world_x, candidate.world_z) > placement.max_slope {
            continue;
        }

        let (shade, root_pressure) =
            competition.pressure(candidate.local_x, candidate.local_z, candidate.genes);
        let candidate_env = PlantEnvironment {
            moisture,
            altitude: normalize_altitude(height, ctx.sea_level),
            river_wetness: ctx.rivers.wetness(candidate.world_x, candidate.world_z),
            shade,
            root_pressure,
        };
        let candidate_pheno = evaluate_phenotype(candidate.genes, species, candidate_env);
        if establishment_roll(ctx.seed, candidate.order) > candidate_pheno.establishment_chance {
            continue;
        }

        grid.insert(candidate.local_x, candidate.local_z);
        competition.insert(
            candidate.local_x,
            candidate.local_z,
            candidate.height,
            stage_to_u8(GrowthStage::Seedling),
            candidate.genes,
        );
        plants.push(Plant {
            local_x: quantize_span(candidate.local_x),
            local_z: quantize_span(candidate.local_z),
            height: quantize_span(candidate.height),
            rotation: quantize_rotation(candidate.rotation),
            species: candidate.species,
            stage: stage_to_u8(GrowthStage::Seedling),
            born_hour: ctx.born_hour,
            genes: candidate.genes,
        });
        accepted += 1;
    }
    accepted
}

/// Uniform spatial grid of plant positions within one chunk, for O(1) spacing
/// queries. Cell size ≥ the largest spacing (8 m) so any plant within `spacing`
/// of a point lies in the point's 3×3 cell neighbourhood.
struct SpacingGrid {
    cells: Vec<Vec<(f32, f32)>>,
}

// Cell size ≥ the largest spacing (8 m) keeps the 3×3 neighbourhood correct;
// 16 m keeps the per-chunk grid small (16×16) to limit allocation churn.
const SPACING_CELL: f32 = 16.0;
const SPACING_GRID_SIDE: i32 = (CHUNK_SIZE_METERS / SPACING_CELL) as i32; // 16

impl SpacingGrid {
    fn cell_of(x: f32, z: f32) -> usize {
        let cx = ((x / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        let cz = ((z / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        (cz * SPACING_GRID_SIDE + cx) as usize
    }

    fn build(plants: &[Plant]) -> Self {
        let mut cells = vec![Vec::new(); (SPACING_GRID_SIDE * SPACING_GRID_SIDE) as usize];
        for plant in plants {
            let x = dequantize_span(plant.local_x);
            let z = dequantize_span(plant.local_z);
            cells[Self::cell_of(x, z)].push((x, z));
        }
        Self { cells }
    }

    fn insert(&mut self, x: f32, z: f32) {
        self.cells[Self::cell_of(x, z)].push((x, z));
    }

    fn any_within(&self, x: f32, z: f32, spacing: f32) -> bool {
        let spacing_sq = spacing * spacing;
        let cx = ((x / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        let cz = ((z / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let nx = cx + dx;
                let nz = cz + dz;
                if nx < 0 || nz < 0 || nx >= SPACING_GRID_SIDE || nz >= SPACING_GRID_SIDE {
                    continue;
                }
                for &(px, pz) in &self.cells[(nz * SPACING_GRID_SIDE + nx) as usize] {
                    let ddx = px - x;
                    let ddz = pz - z;
                    if ddx * ddx + ddz * ddz < spacing_sq {
                        return true;
                    }
                }
            }
        }
        false
    }
}

#[derive(Clone, Copy)]
struct CompetitionEntry {
    x: f32,
    z: f32,
    height: f32,
    stage: u8,
    genes: PlantGenes,
}

struct CompetitionGrid {
    cells: Vec<Vec<CompetitionEntry>>,
}

const COMPETITION_RADIUS: f32 = 24.0;
const COMPETITION_STRENGTH: f32 = 0.55;

impl CompetitionGrid {
    fn build(plants: &[Plant]) -> Self {
        let mut grid = Self {
            cells: vec![Vec::new(); (SPACING_GRID_SIDE * SPACING_GRID_SIDE) as usize],
        };
        for plant in plants {
            grid.insert(
                dequantize_span(plant.local_x),
                dequantize_span(plant.local_z),
                dequantize_span(plant.height),
                plant.stage,
                plant.genes,
            );
        }
        grid
    }

    fn insert(&mut self, x: f32, z: f32, height: f32, stage: u8, genes: PlantGenes) {
        self.cells[SpacingGrid::cell_of(x, z)].push(CompetitionEntry {
            x,
            z,
            height,
            stage,
            genes,
        });
    }

    fn pressure(&self, x: f32, z: f32, genes: PlantGenes) -> (f32, f32) {
        let cx = ((x / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        let cz = ((z / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        let mut canopy = 0.0;
        let mut root = 0.0;
        let candidate_wet = PlantGenes::unit(genes.wet_pref);

        for dz in -2..=2 {
            for dx in -2..=2 {
                let nx = cx + dx;
                let nz = cz + dz;
                if nx < 0 || nz < 0 || nx >= SPACING_GRID_SIDE || nz >= SPACING_GRID_SIDE {
                    continue;
                }
                for entry in &self.cells[(nz * SPACING_GRID_SIDE + nx) as usize] {
                    let ddx = entry.x - x;
                    let ddz = entry.z - z;
                    let distance = (ddx * ddx + ddz * ddz).sqrt();
                    if distance >= COMPETITION_RADIUS {
                        continue;
                    }
                    let falloff = smooth_falloff(distance, COMPETITION_RADIUS);
                    let stage = stage_competition_weight(entry.stage);
                    let capture = PlantGenes::unit(entry.genes.capture);
                    let height_scale = (entry.height / 16.0).clamp(0.25, 1.8);
                    let wet_similarity =
                        1.0 - (candidate_wet - PlantGenes::unit(entry.genes.wet_pref)).abs();
                    canopy += falloff * stage * capture * height_scale;
                    root += falloff * stage * wet_similarity.clamp(0.0, 1.0);
                }
            }
        }

        (
            (canopy * COMPETITION_STRENGTH * 0.55).clamp(0.0, 1.0),
            (root * COMPETITION_STRENGTH * 0.45).clamp(0.0, 1.0),
        )
    }
}

fn stage_competition_weight(stage: u8) -> f32 {
    match stage_from_u8(stage) {
        GrowthStage::Seedling => 0.15,
        GrowthStage::Young => 0.45,
        GrowthStage::Mature => 1.0,
        GrowthStage::Dead => 0.05,
    }
}

fn smooth_falloff(distance: f32, radius: f32) -> f32 {
    let t = (1.0 - distance / radius.max(0.001)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Sim-day index for a spread pass, used to salt the per-day reproduction roll.
/// Rust float→int casts *saturate*, so a direct `f64 as u32` would pin the bucket
/// at `u32::MAX` once the day count overflows `u32` (~1e5 sim-years — unreachable,
/// but then the roll would freeze again). The `i64` hop has headroom the divided
/// hour never reaches, and the final narrowing `as u32` truncates, so consecutive
/// days keep landing in distinct buckets indefinitely.
fn day_bucket(born_hour: f64) -> u32 {
    (born_hour / SPREAD_TICK_HOURS).floor() as i64 as u32
}

/// Mix a per-day `bucket` into a hash sub-key so the same plant draws a fresh,
/// independent value (roll or position jitter) on each successive spread pass.
fn bucket_salt(base: u32, bucket: u32) -> u32 {
    base ^ bucket.wrapping_mul(0x9E37_79B9)
}

fn spread_roll(seed: u32, canon: IVec2, plant_index: u32, bucket: u32) -> f32 {
    hash_unit(
        seed.wrapping_add(4001),
        canon,
        bucket_salt(plant_index, bucket),
    )
}

fn spread_seed_count(seed: u32, canon: IVec2, plant_index: u32, bucket: u32) -> u32 {
    1 + (hash_unit(
        seed.wrapping_add(4002),
        canon,
        bucket_salt(plant_index, bucket),
    ) * 2.0)
        .floor() as u32
}

fn establishment_roll(seed: u32, order: u64) -> f32 {
    hash_to_unit_float(hash4(
        seed.wrapping_add(4401),
        (order >> 32) as u32,
        order as u32,
        0,
    ))
}

fn hash_unit(seed: u32, canon: IVec2, sub: u32) -> f32 {
    hash_to_unit_float(hash4(seed, canon.x as u32, canon.y as u32, sub))
}

/// Build the per-canonical-chunk house index by sampling the continuous
/// heightmap — the same terrain source [`land_chunk_candidates`] validates
/// against — so the houses the spread pass fences seedlings away from are
/// reconstructed identically on every world, locally generated or downloaded,
/// without persisting them in the base snapshot. Returns one XZ list per
/// canonical chunk, indexed `cz * WORLD_SIZE_CHUNKS + cx`.
///
/// `threads` caps Rayon parallelism: `Some(n)` runs in a pool of `n` threads so
/// this step honours the same budget as the rest of `generate_base` (rather than
/// oversubscribing the global pool during world creation); `None` uses the
/// ambient pool, for the lighter snapshot-load path.
fn build_house_index(
    seed: u32,
    houses_config: &HousesConfig,
    biome_config: &BiomeConfig,
    heightmap: &Heightmap,
    sea_level: f32,
    threads: Option<usize>,
) -> Vec<Vec<Vec2>> {
    let n = WORLD_SIZE_CHUNKS;
    let total = (n as usize) * (n as usize);
    let build = |idx: usize| -> Vec<Vec2> {
        let cx = (idx as i32) % n;
        let cz = (idx as i32) / n;
        let origin_x = cx as f32 * CHUNK_SIZE_METERS;
        let origin_z = cz as f32 * CHUNK_SIZE_METERS;
        place_houses(
            seed,
            houses_config,
            IVec2::new(cx, cz),
            |local_x, local_z| {
                // Mirror `HousesLayer`'s grid validation, but sample the continuous
                // heightmap and classify the biome from it (as the landing pass
                // does) rather than the per-chunk terrain/biome grids. Biome first;
                // `slope_at`'s four extra samples are paid only on grassland.
                let wx = origin_x + local_x;
                let wz = origin_z + local_z;
                let height = heightmap.sample_height(wx, wz);
                let moisture = heightmap.sample_moisture(wx, wz);
                if classify(height, moisture, biome_config) != Biome::Grassland {
                    return None;
                }
                if slope_at(heightmap, wx, wz) <= houses_config.max_slope
                    && height >= sea_level
                    && (houses_config.height_min..=houses_config.height_max).contains(&height)
                {
                    Some(height)
                } else {
                    None
                }
            },
        )
        .into_iter()
        .map(|site| Vec2::new(site.world_x, site.world_z))
        .collect()
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let run = || (0..total).into_par_iter().map(build).collect::<Vec<_>>();
        match threads {
            Some(t) => {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(t.max(1))
                    .build()
                    .ok();
                match &pool {
                    Some(pool) => pool.install(run),
                    None => run(),
                }
            }
            None => run(),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = threads;
        (0..total).map(build).collect()
    }
}

/// Minimum clearance, in metres, a spread seedling of `kind` must keep from any
/// house — so reproducing trees and shrubs don't slowly grow up inside the
/// buildings. Trees get a wider berth than shrubs.
fn house_clearance(kind: &str) -> f32 {
    if kind == "shrub" {
        5.0
    } else {
        10.0
    }
}

/// Whether `(world_x, world_z)` lies within `clearance` of any house. Houses are
/// indexed by canonical chunk, so only the candidate's chunk and its eight
/// (wrapped) neighbours can hold one close enough — distances are measured on the
/// torus so the check is correct across the world seam.
fn near_house(houses: &[Vec<Vec2>], world_x: f32, world_z: f32, clearance: f32) -> bool {
    let world = WORLD_SIZE_METERS as f32;
    let half = world * 0.5;
    let clearance_sq = clearance * clearance;
    let n = WORLD_SIZE_CHUNKS;
    let cx = ((world_x / CHUNK_SIZE_METERS).floor() as i32).rem_euclid(n);
    let cz = ((world_z / CHUNK_SIZE_METERS).floor() as i32).rem_euclid(n);
    for dz in -1..=1 {
        for dx in -1..=1 {
            let nx = (cx + dx).rem_euclid(n);
            let nz = (cz + dz).rem_euclid(n);
            // The real index always spans the full canonical grid; `get` only
            // guards the smaller hand-built worlds used in tests.
            let Some(cell) = houses.get((nz * n + nx) as usize) else {
                continue;
            };
            for house in cell {
                // Shortest separation on the torus, so a neighbour chunk wrapped
                // across the seam still measures as adjacent rather than a lap away.
                let mut ddx = (house.x - world_x).abs();
                if ddx > half {
                    ddx = world - ddx;
                }
                let mut ddz = (house.y - world_z).abs();
                if ddz > half {
                    ddz = world - ddz;
                }
                if ddx * ddx + ddz * ddz < clearance_sq {
                    return true;
                }
            }
        }
    }
    false
}

fn slope_at(heightmap: &Heightmap, x: f32, z: f32) -> f32 {
    let d = 1.75;
    let hx0 = heightmap.sample_height(x - d, z);
    let hx1 = heightmap.sample_height(x + d, z);
    let hz0 = heightmap.sample_height(x, z - d);
    let hz1 = heightmap.sample_height(x, z + d);
    let dx = (hx1 - hx0) / (2.0 * d);
    let dz = (hz1 - hz0) / (2.0 * d);
    (dx * dx + dz * dz).sqrt()
}

fn min_spacing(kind: &str) -> f32 {
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

const BIOME_SLOTS: usize = 5;

/// Order biomes are reported in. Output order, independent of enum discriminant.
const BIOME_ORDER: [Biome; BIOME_SLOTS] = [
    Biome::Forest,
    Biome::Grassland,
    Biome::Desert,
    Biome::Rock,
    Biome::Snow,
];

/// Stable per-biome slot for the fill tallies. An exhaustive match (rather than
/// `biome as usize`) so adding a `Biome` variant is a compile error here, not an
/// out-of-bounds index at runtime.
fn biome_slot(biome: Biome) -> usize {
    match biome {
        Biome::Forest => 0,
        Biome::Grassland => 1,
        Biome::Desert => 2,
        Biome::Rock => 3,
        Biome::Snow => 4,
    }
}

/// Inverse of [`biome_slot`], for decoding the base-world snapshot. Unknown codes
/// fall back to `Snow` rather than panicking on a corrupt byte.
fn biome_from_slot(slot: u8) -> Biome {
    match slot {
        0 => Biome::Forest,
        1 => Biome::Grassland,
        2 => Biome::Desert,
        3 => Biome::Rock,
        _ => Biome::Snow,
    }
}

// ---------------------------------------------------------------------------
// Spread persistence — compact little-endian blob of the per-chunk spread suffix
// ---------------------------------------------------------------------------

const SPREAD_MAGIC: u32 = 0x504c_4e54; // "PLNT"
const SPREAD_VERSION: u32 = 2;
/// Serialized size of one plant: u16×4 + u8×2 + f32 + eight gene bytes.
const PLANT_BYTES: usize = 22;

// ---------------------------------------------------------------------------
// Base-world snapshot — full per-chunk base flora + biome/map, a cache of the
// expensive `generate_base` pass keyed by the generation inputs.
//
// v3 stores plants as three Brotli-compressed columns (meta / position-deltas /
// attributes) instead of a flat 14-byte record. Combined with reduced field
// precision and Morton-delta positions this takes the default world from
// ~190 MiB to ~31 MiB. Base plants are snapped to this grid at generation time
// (`Plant::pack`) so a downloaded snapshot is bit-identical to a local one.
// ---------------------------------------------------------------------------

const BASE_MAGIC: u32 = 0x5742_4153; // "WBAS"
const BASE_VERSION: u32 = 3;
const BASE_ATTR_BYTES: usize = 3 + PlantGenes::BYTE_LEN;

/// Bits of position precision stored per axis, over the 256 m chunk: 10 bits →
/// a 0.25 m grid. `POS_SHIFT` is how far that sits below the resident `u16`
/// (`local_x`/`local_z`) span, so a stored code is `local >> POS_SHIFT`.
const POS_BITS: u32 = 10;
const POS_SHIFT: u16 = 16 - POS_BITS as u16; // 6
/// Number of distinct Morton cells on the position grid (`2^(2*POS_BITS)`), the
/// exclusive upper bound for a stored position code and a hard ceiling on plants
/// per chunk during load.
const MORTON_CELLS: usize = 1 << (2 * POS_BITS);

/// Plant height is stored as a `u8` over `[0, HEIGHT_RANGE_M)` → a 0.125 m step.
/// The tallest default species tops out at 25 m, so 32 m leaves headroom; a
/// taller plant would clamp (cosmetic only — height never feeds spread).
const HEIGHT_RANGE_M: f32 = 32.0;

/// Brotli window (`lgwin`, 10–24) for the snapshot sections; 24 (16 MiB) lets the
/// compressor reach back across a whole column. Quality is chosen per call —
/// see [`PlantWorld::DOWNLOAD_QUALITY`] / [`PlantWorld::LOCAL_QUALITY`].
const BASE_BROTLI_LGWIN: u32 = 24;

/// Cap on a single decompressed section, to bound allocation from a corrupt or
/// hostile header before the bytes are trusted.
const BASE_SECTION_MAX_BYTES: usize = 512 * 1024 * 1024;

/// Mask a quantized position down to the `POS_BITS` storage grid (clears the low
/// `POS_SHIFT` bits) so generation and snapshot round-trips agree exactly.
fn snap_pos(v: u16) -> u16 {
    v & !((1u16 << POS_SHIFT) - 1)
}

/// Quantize a height in metres to the 8-bit storage range.
fn pack_height(height_m: f32) -> u8 {
    (height_m / HEIGHT_RANGE_M * 256.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Inverse of [`pack_height`], back into the resident `u16` height span so
/// [`dequantize_span`] reproduces the stored metres.
fn unpack_height(h8: u8) -> u16 {
    quantize_span(h8 as f32 / 256.0 * HEIGHT_RANGE_M)
}

/// Quantize a rotation in radians to 8 bits over `[0, TAU)` (wraps at TAU).
fn pack_rotation(radians: f32) -> u8 {
    ((radians.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 256.0).round() as i64
        & 0xFF) as u8
}

/// Interleave the low `POS_BITS` of `x` and `z` into a Morton (Z-order) code.
/// Sorting a chunk by this keeps spatially-near plants adjacent, so their
/// delta-coded positions stay small. A plain bit loop — only run once per plant
/// at generation/serialization, never on a hot path.
fn morton10(x: u16, z: u16) -> u32 {
    let mut code = 0u32;
    for i in 0..POS_BITS {
        code |= ((x as u32 >> i) & 1) << (2 * i);
        code |= ((z as u32 >> i) & 1) << (2 * i + 1);
    }
    code
}

/// Inverse of [`morton10`].
fn unmorton10(code: u32) -> (u16, u16) {
    let mut x = 0u16;
    let mut z = 0u16;
    for i in 0..POS_BITS {
        x |= (((code >> (2 * i)) & 1) as u16) << i;
        z |= (((code >> (2 * i + 1)) & 1) as u16) << i;
    }
    (x, z)
}

/// Sort a chunk's plants into canonical Morton order (stable, so equal cells keep
/// generation order). This both shrinks the snapshot and pins each plant's index
/// within its chunk — which spread RNG reads — so every client agrees.
fn sort_chunk_canonical(plants: &mut [Plant]) {
    plants.sort_by_key(|p| morton10(p.local_x >> POS_SHIFT, p.local_z >> POS_SHIFT));
}

/// LEB128 unsigned varint, little-endian groups of 7 bits. Morton deltas are
/// mostly tiny, so this is where the position stream shrinks before compression.
fn write_varint(buf: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn read_varint(cur: &mut Cursor<'_>) -> anyhow::Result<u32> {
    let mut result = 0u32;
    let mut shift = 0u32;
    loop {
        if shift >= 32 {
            anyhow::bail!("varint too long");
        }
        let byte = cur.read_u8()?;
        result |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}

/// Brotli-compress a section body. Writing to a `Vec` is infallible, so this
/// can't error.
fn brotli_compress(data: &[u8], quality: u32) -> Vec<u8> {
    use std::io::Write;
    let mut out = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut out, 4096, quality, BASE_BROTLI_LGWIN);
        writer
            .write_all(data)
            .expect("brotli compression into a Vec is infallible");
    }
    out
}

/// Brotli-decompress, refusing anything that expands past `max_len` so a crafted
/// stream can't blow up memory.
fn brotli_decompress(data: &[u8], max_len: usize) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    brotli::Decompressor::new(data, 4096)
        .take(max_len as u64 + 1)
        .read_to_end(&mut out)?;
    if out.len() > max_len {
        anyhow::bail!("decompressed section exceeds declared length");
    }
    Ok(out)
}

/// Append a length-framed, Brotli-compressed section at the given `quality`:
/// `uncompressed_len(u32)`, `compressed_len(u32)`, then the compressed bytes.
fn write_section(buf: &mut Vec<u8>, data: &[u8], quality: u32) {
    let comp = brotli_compress(data, quality);
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&comp);
}

/// Read and decompress one [`write_section`] frame, validating both lengths.
fn read_section(cur: &mut Cursor<'_>) -> anyhow::Result<Vec<u8>> {
    let uncomp_len = cur.read_u32()? as usize;
    let comp_len = cur.read_u32()? as usize;
    if uncomp_len > BASE_SECTION_MAX_BYTES {
        anyhow::bail!("section length {uncomp_len} exceeds {BASE_SECTION_MAX_BYTES}");
    }
    if comp_len > cur.remaining() {
        anyhow::bail!("compressed section {comp_len} exceeds remaining data");
    }
    let comp = cur.take(comp_len)?;
    let out = brotli_decompress(comp, uncomp_len)?;
    if out.len() != uncomp_len {
        anyhow::bail!(
            "section length mismatch: decoded {} want {uncomp_len}",
            out.len()
        );
    }
    Ok(out)
}

/// Read one section frame's lengths and hand back its compressed bytes (owned)
/// plus the declared decompressed length, advancing past the frame. Same framing
/// as [`read_section`] but it defers decompression to [`SectionDecoder`] so the
/// incremental [`BaseLoader`] can spread the decode over several event-loop turns.
#[cfg(any(target_arch = "wasm32", test))]
fn take_section_frame(cur: &mut Cursor<'_>) -> anyhow::Result<(Vec<u8>, usize)> {
    let uncomp_len = cur.read_u32()? as usize;
    let comp_len = cur.read_u32()? as usize;
    if uncomp_len > BASE_SECTION_MAX_BYTES {
        anyhow::bail!("section length {uncomp_len} exceeds {BASE_SECTION_MAX_BYTES}");
    }
    if comp_len > cur.remaining() {
        anyhow::bail!("compressed section {comp_len} exceeds remaining data");
    }
    Ok((cur.take(comp_len)?.to_vec(), uncomp_len))
}

/// Brotli-decompresses one section a bounded number of bytes at a time, so a large
/// section can be decoded across several [`BaseLoader::step`] calls instead of one
/// blocking burst (the web build has no worker thread to absorb it).
#[cfg(any(target_arch = "wasm32", test))]
struct SectionDecoder {
    inner: brotli::Decompressor<std::io::Cursor<Vec<u8>>>,
    out: Vec<u8>,
    declared: usize,
}

/// Upper bound on the output buffer pre-reserved from a section's declared length.
/// Reserving the known size up front avoids repeated reallocations as the decode
/// grows, but the cap keeps a corrupt or hostile `declared` (up to
/// [`BASE_SECTION_MAX_BYTES`]) from forcing a huge allocation before any bytes are
/// validated — real sections are well under this.
#[cfg(any(target_arch = "wasm32", test))]
const SECTION_DECODE_RESERVE_CAP: usize = 64 * 1024 * 1024;

#[cfg(any(target_arch = "wasm32", test))]
impl SectionDecoder {
    fn new(comp: Vec<u8>, declared: usize) -> Self {
        Self {
            inner: brotli::Decompressor::new(std::io::Cursor::new(comp), 4096),
            out: Vec::with_capacity(declared.min(SECTION_DECODE_RESERVE_CAP)),
            declared,
        }
    }

    /// Fraction of this section decoded so far, `0.0..=1.0`, for smooth progress.
    fn fraction(&self) -> f32 {
        if self.declared == 0 {
            1.0
        } else {
            self.out.len() as f32 / self.declared as f32
        }
    }

    /// Decode up to `budget` more bytes. Returns `Ok(true)` once the whole section
    /// is decoded and its length validated against the declared size; the same
    /// over-/under-run checks as [`read_section`], applied incrementally.
    fn step(&mut self, budget: usize) -> anyhow::Result<bool> {
        use std::io::Read;
        let mut chunk = [0u8; 64 * 1024];
        let mut produced = 0usize;
        while produced < budget {
            let n = self.inner.read(&mut chunk)?;
            if n == 0 {
                if self.out.len() != self.declared {
                    anyhow::bail!(
                        "section length mismatch: decoded {} want {}",
                        self.out.len(),
                        self.declared
                    );
                }
                return Ok(true);
            }
            if self.out.len() + n > self.declared {
                anyhow::bail!("decompressed section exceeds declared length");
            }
            self.out.extend_from_slice(&chunk[..n]);
            produced += n;
        }
        Ok(false)
    }
}

/// One unit of progress from [`BaseLoader::step`]. The finished world is boxed so
/// the common `Working` value stays small (the load returns one per step).
#[cfg(any(target_arch = "wasm32", test))]
pub enum LoadStep {
    /// Still working; `0.0..=1.0` is the fraction of the load completed.
    Working(f32),
    /// The world is fully reconstructed.
    Done(Box<PlantWorld>),
}

/// Resumable loader for a base-world snapshot. Construction validates the header,
/// rejecting a stale or foreign snapshot exactly like
/// [`PlantWorld::from_base_snapshot`]; [`step`](Self::step) then decodes the three
/// Brotli sections and rebuilds the per-chunk plants a bounded amount at a time.
///
/// Native loads stay one-shot on a worker thread via
/// [`PlantWorld::from_base_snapshot`]. This incremental path exists for the web
/// build, which has no worker thread: stepping it from the async loader and
/// yielding to the browser between steps keeps the loading screen animating
/// instead of freezing the tab for the whole decode-and-rebuild.
#[cfg(any(target_arch = "wasm32", test))]
pub struct BaseLoader {
    seed: u32,
    config: GameConfig,
    registry: Arc<PlantRegistry>,
    rivers: Arc<RiverField>,
    total: usize,
    /// Compressed sections still to decode, in order: meta, pos, attr.
    pending: std::collections::VecDeque<(Vec<u8>, usize)>,
    current: Option<SectionDecoder>,
    /// Decoded sections, accumulated in the same meta/pos/attr order.
    decoded: Vec<Vec<u8>>,
    rebuild: Option<RebuildState>,
}

/// Cursor offsets and accumulating output for the per-chunk rebuild phase.
#[cfg(any(target_arch = "wasm32", test))]
struct RebuildState {
    meta_off: usize,
    pos_off: usize,
    attr_off: usize,
    next_chunk: usize,
    chunks: Vec<Vec<Plant>>,
    biome: Vec<Biome>,
    cell_bytes: Vec<u8>,
}

#[cfg(any(target_arch = "wasm32", test))]
impl BaseLoader {
    /// Bytes decoded per [`step`](Self::step) during the decode phase.
    const DECODE_BUDGET: usize = 4 * 1024 * 1024;
    /// Chunks rebuilt per [`step`](Self::step) during the rebuild phase.
    const REBUILD_BATCH: usize = 1024;

    /// Validate the header and stage the three compressed sections. Returns `Err`
    /// on a bad/foreign/corrupt header (caller falls back to local generation),
    /// matching [`PlantWorld::from_base_snapshot`]'s reject-and-regenerate path.
    pub fn new(
        bytes: &[u8],
        gen_key: u64,
        expected_seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
    ) -> anyhow::Result<Self> {
        let mut cur = Cursor::new(bytes);
        if cur.read_u32()? != BASE_MAGIC {
            anyhow::bail!("bad magic");
        }
        if cur.read_u32()? != BASE_VERSION {
            anyhow::bail!("unsupported version");
        }
        let key = cur.read_u64()?;
        if key != gen_key {
            anyhow::bail!("generation key {key:#018x} does not match {gen_key:#018x}");
        }
        let seed = cur.read_u32()?;
        if seed != expected_seed {
            anyhow::bail!("seed {seed} does not match world seed {expected_seed}");
        }
        let n = cur.read_u32()?;
        if n != WORLD_SIZE_CHUNKS as u32 {
            anyhow::bail!("world size {n} does not match {WORLD_SIZE_CHUNKS}");
        }
        let pos_bits = cur.read_u8()?;
        if pos_bits as u32 != POS_BITS {
            anyhow::bail!("position precision {pos_bits} does not match {POS_BITS}");
        }
        cur.skip(3)?; // reserved
        let total = (n as usize) * (n as usize);

        let mut pending = std::collections::VecDeque::with_capacity(3);
        pending.push_back(take_section_frame(&mut cur)?); // meta
        pending.push_back(take_section_frame(&mut cur)?); // pos
        pending.push_back(take_section_frame(&mut cur)?); // attr

        Ok(Self {
            seed,
            config: config.clone(),
            registry,
            rivers,
            total,
            pending,
            current: None,
            decoded: Vec::with_capacity(3),
            rebuild: None,
        })
    }

    /// Advance the load by a bounded amount of work and report progress. The
    /// decode phase owns the first half of the fraction, the rebuild the second.
    pub fn step(&mut self) -> anyhow::Result<LoadStep> {
        // Phase 1: decode the three sections, one bounded slice per call.
        if self.decoded.len() < 3 {
            if self.current.is_none() {
                let (comp, declared) = self.pending.pop_front().expect("pending section");
                self.current = Some(SectionDecoder::new(comp, declared));
            }
            // Fold the in-flight section's own progress into the fraction so the
            // bar still moves on a section that takes several steps to decode.
            let partial = if self.current.as_mut().unwrap().step(Self::DECODE_BUDGET)? {
                self.decoded.push(self.current.take().unwrap().out);
                0.0
            } else {
                self.current.as_ref().unwrap().fraction()
            };
            // The three sections span the first half of the bar (each ~1/6).
            return Ok(LoadStep::Working(
                (self.decoded.len() as f32 + partial) / 6.0,
            ));
        }

        // Phase 2: rebuild chunks in batches. Own the state for the call so the
        // section borrows below don't collide with the `Option` field.
        let total = self.total;
        let mut rb = self.rebuild.take().unwrap_or_else(|| RebuildState {
            meta_off: 0,
            pos_off: 0,
            attr_off: 0,
            next_chunk: 0,
            chunks: Vec::with_capacity(total),
            biome: Vec::with_capacity(total),
            cell_bytes: Vec::with_capacity(total),
        });
        let mut meta_cur = Cursor {
            buf: &self.decoded[0],
            pos: rb.meta_off,
        };
        let mut pos_cur = Cursor {
            buf: &self.decoded[1],
            pos: rb.pos_off,
        };
        let mut attr_cur = Cursor {
            buf: &self.decoded[2],
            pos: rb.attr_off,
        };
        let end = (rb.next_chunk + Self::REBUILD_BATCH).min(total);
        for _ in rb.next_chunk..end {
            let (plants, b, cell) = parse_base_chunk(&mut meta_cur, &mut pos_cur, &mut attr_cur)?;
            rb.chunks.push(plants);
            rb.biome.push(b);
            rb.cell_bytes.push(cell);
        }
        rb.meta_off = meta_cur.pos;
        rb.pos_off = pos_cur.pos;
        rb.attr_off = attr_cur.pos;
        rb.next_chunk = end;

        if rb.next_chunk >= total {
            let world = PlantWorld::assemble_base(
                rb.chunks,
                rb.biome,
                rb.cell_bytes,
                self.seed,
                &self.config,
                Arc::clone(&self.registry),
                Arc::clone(&self.rivers),
            );
            return Ok(LoadStep::Done(Box::new(world)));
        }
        let frac = 0.5 + 0.5 * (rb.next_chunk as f32 / total.max(1) as f32);
        self.rebuild = Some(rb);
        Ok(LoadStep::Working(frac))
    }
}

fn write_plant(buf: &mut Vec<u8>, p: &Plant) {
    buf.extend_from_slice(&p.local_x.to_le_bytes());
    buf.extend_from_slice(&p.local_z.to_le_bytes());
    buf.extend_from_slice(&p.height.to_le_bytes());
    buf.extend_from_slice(&p.rotation.to_le_bytes());
    buf.push(p.species);
    buf.push(p.stage);
    buf.extend_from_slice(&p.born_hour.to_le_bytes());
    buf.extend_from_slice(&p.genes.to_bytes());
}

fn read_plant(cur: &mut Cursor<'_>) -> anyhow::Result<Plant> {
    Ok(Plant {
        local_x: cur.read_u16()?,
        local_z: cur.read_u16()?,
        height: cur.read_u16()?,
        rotation: cur.read_u16()?,
        species: cur.read_u8()?,
        stage: cur.read_u8()?,
        born_hour: cur.read_f32()?,
        genes: PlantGenes::from_bytes(cur.read_array()?),
    })
}

/// Minimal bounds-checked little-endian reader over a byte slice.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len());
        let Some(end) = end else {
            anyhow::bail!("unexpected end of data");
        };
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, n: usize) -> anyhow::Result<()> {
        self.take(n)?;
        Ok(())
    }

    fn read_u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_array<const N: usize>(&mut self) -> anyhow::Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn read_u16(&mut self) -> anyhow::Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> anyhow::Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> anyhow::Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_f32(&mut self) -> anyhow::Result<f32> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// A plant's full life timeline in age-space (sim-hours since `born_hour`).
/// `dead_at`/`gone_at` are `INFINITY` for immortal species (`lifespan_hours <=
/// 0`). Death and despawn are analytic, exactly like growth: the timeline is a
/// pure function of the plant's stable identity, so a reloaded world removes
/// the same plants a live one did — no tombstones in the save.
#[derive(Clone, Copy, Debug)]
struct LifeSchedule {
    young_at: f64,
    mature_at: f64,
    dead_at: f64,
    gone_at: f64,
}

impl LifeSchedule {
    /// Cached stage byte for a plant of this schedule at `age`, or `None` once
    /// it is past `gone_at` and should despawn.
    fn stage_at(&self, age: f64) -> Option<u8> {
        if age >= self.gone_at {
            None
        } else if age >= self.dead_at {
            Some(DEAD)
        } else if age >= self.mature_at {
            Some(MATURE)
        } else if age >= self.young_at {
            Some(stage_to_u8(GrowthStage::Young))
        } else {
            Some(stage_to_u8(GrowthStage::Seedling))
        }
    }

    /// First threshold strictly ahead of `age`, in age-space (`INFINITY` when
    /// nothing is left to happen — a mature immortal).
    fn next_threshold(&self, age: f64) -> f64 {
        if age < self.young_at {
            self.young_at
        } else if age < self.mature_at {
            self.mature_at
        } else if age < self.dead_at {
            self.dead_at
        } else {
            self.gone_at
        }
    }
}

/// Deterministic per-plant unit hash for death timing, keyed on stable identity
/// (world seed, canonical chunk, quantized position, birth time, species) —
/// deliberately *not* the plant's index, which shifts as neighbours despawn.
fn death_unit(seed: u32, chunk_idx: usize, plant: &Plant, salt: u32) -> f32 {
    let pos = ((plant.local_x as u32) << 16) | plant.local_z as u32;
    hash_to_unit_float(hash4(
        seed.wrapping_add(salt),
        chunk_idx as u32,
        pos,
        plant.born_hour.to_bits() ^ plant.species as u32,
    ))
}

/// Analytic life timeline for a packed plant. `is_base` marks deterministic
/// base-flora plants (the prefix of each chunk's list): they are **mature from
/// creation** (their growth thresholds are zero — a new world must not demote
/// the baked forest to seedlings), and instead of the spread plants' ±25%
/// lifespan jitter they draw a uniform fraction of a full lifespan, so the
/// starting forest dies as a steady trickle spread over one lifespan, never a
/// wavefront.
fn life_schedule(
    seed: u32,
    chunk_idx: usize,
    plant: &Plant,
    species: &crate::world_core::herbarium::PlantSpeciesInfo,
    is_base: bool,
    env: PlantEnvironment,
) -> LifeSchedule {
    let phenotype = evaluate_phenotype(plant.genes, species, env);
    let maturity_scale = phenotype.maturity_scale as f64;
    let (young_at, mature_at) = if is_base {
        (0.0, 0.0)
    } else {
        let young_at = species.placement.seedling_hours.max(0.0) as f64 * maturity_scale;
        (
            young_at,
            young_at + species.placement.young_hours.max(0.0) as f64 * maturity_scale,
        )
    };
    let lifespan = species.placement.lifespan_hours;
    if lifespan <= 0.0 {
        return LifeSchedule {
            young_at,
            mature_at,
            dead_at: f64::INFINITY,
            gone_at: f64::INFINITY,
        };
    }
    let life_factor = if is_base {
        death_unit(seed, chunk_idx, plant, 4301)
    } else {
        0.75 + 0.5 * death_unit(seed, chunk_idx, plant, 4301)
    };
    let stress_life = (1.0 - phenotype.stress as f64 * 0.35).clamp(0.35, 1.0);
    let lifespan_scale = (phenotype.lifespan_scale as f64 * stress_life).clamp(0.25, 2.0);
    let snag_factor = 0.75 + 0.5 * death_unit(seed, chunk_idx, plant, 4302);
    let dead_at = mature_at + lifespan as f64 * life_factor as f64 * lifespan_scale;
    LifeSchedule {
        young_at,
        mature_at,
        dead_at,
        gone_at: dead_at + (species.placement.snag_hours.max(0.0) * snag_factor) as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_core::herbarium::Herbarium;
    use crate::world_core::storage::Storage as _;

    fn registry() -> Arc<PlantRegistry> {
        Arc::new(PlantRegistry::from_herbarium(&Herbarium::default_seeded()))
    }

    // A small PlantWorld with the given per-chunk plant lists; terrain/rules come
    // from the default config so spread validation samples the real heightmap.
    fn test_world(chunks: Vec<Vec<Plant>>, reg: Arc<PlantRegistry>) -> PlantWorld {
        let config = GameConfig::default();
        let population = chunks.iter().map(Vec::len).sum();
        let populated_chunks = chunks.iter().filter(|c| !c.is_empty()).count();
        let immature = chunks
            .iter()
            .map(|c| c.iter().filter(|p| p.stage < MATURE).count() as u32)
            .collect();
        let saturated = vec![false; chunks.len()];
        let base_count = chunks.iter().map(|c| c.len() as u32).collect();
        let biome = vec![Biome::Forest; chunks.len()];
        let cell_bytes = vec![0u8; chunks.len()];
        let next_event = vec![0.0; chunks.len()];
        let houses = vec![Vec::new(); chunks.len()];
        PlantWorld {
            chunks,
            immature,
            next_event,
            growth_hours: 0.0,
            saturated,
            base_count,
            biome,
            cell_bytes,
            registry: reg,
            heightmap: Heightmap::new(7, config.heightmap.clone()),
            rivers: Arc::new(RiverField::empty()),
            houses,
            biome_config: config.biome.clone(),
            sea_level: config.sea_level,
            seed: 7,
            population,
            populated_chunks,
            last_spread_added: 0,
            biome_fill: Vec::new(),
        }
    }

    // In-memory storage with binary support, for persistence round-trips.
    #[derive(Default)]
    struct MemBytes {
        bytes: std::cell::RefCell<std::collections::HashMap<String, Vec<u8>>>,
    }
    impl crate::world_core::storage::Storage for MemBytes {
        fn load(&self, _key: &str) -> Option<String> {
            None
        }
        fn save(&self, _key: &str, _data: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn load_bytes(&self, key: &str) -> Option<Vec<u8>> {
            self.bytes.borrow().get(key).cloned()
        }
        fn save_bytes(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
            self.bytes
                .borrow_mut()
                .insert(key.to_string(), data.to_vec());
            Ok(())
        }
    }

    fn seedling(local_x: f32, born_hour: f32) -> Plant {
        Plant {
            local_x: quantize_span(local_x),
            local_z: quantize_span(50.0),
            height: quantize_span(4.0),
            rotation: quantize_rotation(1.0),
            species: 0,
            stage: stage_to_u8(GrowthStage::Seedling),
            born_hour,
            genes: PlantGenes::from_bytes([11, 22, 33, 44, 55, 66, 77, 88]),
        }
    }

    // A mature base plant already snapped to the base-snapshot storage grid,
    // exactly as `Plant::pack` produces — so a base-snapshot round-trip is
    // bit-identical and `assert_eq!` on whole chunks holds.
    fn grid_plant(local_x: f32, local_z: f32, species: u8) -> Plant {
        let h8 = pack_height(4.0);
        let r8 = pack_rotation(1.0);
        Plant {
            local_x: snap_pos(quantize_span(local_x)),
            local_z: snap_pos(quantize_span(local_z)),
            height: unpack_height(h8),
            rotation: (r8 as u16) << 8,
            species,
            stage: MATURE,
            born_hour: 0.0,
            genes: PlantGenes::from_bytes([species.wrapping_add(1), 21, 42, 63, 84, 105, 126, 147]),
        }
    }

    fn neutral_env() -> PlantEnvironment {
        PlantEnvironment {
            moisture: 0.5,
            altitude: 0.5,
            river_wetness: 0.0,
            shade: 0.0,
            root_pressure: 0.0,
        }
    }

    #[test]
    fn spread_state_round_trips_through_save_and_load() {
        let reg = registry();
        // A world whose chunk 1 has one base plant plus two spread seedlings.
        let base = Plant {
            stage: MATURE,
            ..seedling(10.0, 0.0)
        };
        let mut world = test_world(vec![Vec::new(), vec![base], Vec::new()], Arc::clone(&reg));
        // base_count was set to current lengths, so anything appended now is spread.
        world.chunks[1].push(seedling(20.0, 24.0));
        world.chunks[1].push(seedling(30.0, 24.0));
        world.immature[1] = 2;
        world.population += 2;

        let storage = MemBytes::default();
        world.save_spread(&storage).unwrap();

        // Reload: a fresh world with only the base, then apply the saved spread.
        let mut reloaded = test_world(vec![Vec::new(), vec![base], Vec::new()], reg);
        let restored = reloaded.apply_saved_spread(&storage);

        assert_eq!(restored, 2);
        assert_eq!(reloaded.population, world.population);
        assert_eq!(reloaded.chunks[1].len(), 3);
        assert_eq!(reloaded.immature[1], 2);
        // The spread suffix is byte-identical.
        assert_eq!(&reloaded.chunks[1][1..], &world.chunks[1][1..]);
        // Saturation is seeded from maturity so a settled resume is cheap: the
        // all-mature empty chunks are saturated, the chunk with seedlings is not.
        assert!(reloaded.saturated[0]);
        assert!(!reloaded.saturated[1]);
        assert!(reloaded.saturated[2]);
    }

    #[test]
    fn base_snapshot_round_trips_and_rebuilds_indices() {
        let reg = registry();
        // A full-size world (the snapshot format is fixed to WORLD_SIZE_CHUNKS²)
        // with a few populated chunks. Base flora is always mature and snapped to
        // the storage grid, so plant lists round-trip bit-identically once each
        // chunk is in canonical Morton order.
        let total = (WORLD_SIZE_CHUNKS as usize) * (WORLD_SIZE_CHUNKS as usize);
        let mut chunks = vec![Vec::new(); total];
        chunks[5].push(grid_plant(10.0, 50.0, 0));
        chunks[5].push(grid_plant(20.0, 80.0, 3));
        chunks[total - 1].push(grid_plant(10.0, 50.0, 1));
        for c in &mut chunks {
            sort_chunk_canonical(c);
        }
        let world = test_world(chunks, Arc::clone(&reg));

        let config = GameConfig::default();
        let gen_key = 0xABCD_1234_5678_9A00u64;

        // Brotli decode is quality-agnostic, so a fast local cache (q1) and the
        // shipped download (q10) share the v2 layout and round-trip identically.
        for quality in [PlantWorld::LOCAL_QUALITY, PlantWorld::DOWNLOAD_QUALITY] {
            let bytes = world.serialize_base(gen_key, quality);
            // The blob is the v2 compressed format, not a flat plant dump.
            assert_eq!(&bytes[0..4], &BASE_MAGIC.to_le_bytes());
            assert_eq!(&bytes[4..8], &BASE_VERSION.to_le_bytes());

            let restored = PlantWorld::from_base_snapshot(
                &bytes,
                gen_key,
                world.seed,
                &config,
                Arc::clone(&reg),
                Arc::new(RiverField::empty()),
            )
            .expect("a matching snapshot loads");
            assert_eq!(restored.population(), world.population());
            assert_eq!(restored.populated_chunks(), world.populated_chunks());
            // Plant lists round-trip byte-for-byte; derived indices are rebuilt.
            // Base plants are all mature, so nothing is immature after a load.
            assert_eq!(restored.chunks[5], world.chunks[5]);
            assert_eq!(restored.base_count[5], 2);
            assert_eq!(restored.immature[5], 0);
            assert_eq!(restored.chunks[total - 1], world.chunks[total - 1]);

            // A snapshot is rejected (falls back to generation) when the
            // generation key or the seed does not match.
            assert!(
                PlantWorld::from_base_snapshot(
                    &bytes,
                    gen_key ^ 1,
                    world.seed,
                    &config,
                    registry(),
                    Arc::new(RiverField::empty()),
                )
                .is_none(),
                "a changed generation key must invalidate the cache"
            );
            assert!(
                PlantWorld::from_base_snapshot(
                    &bytes,
                    gen_key,
                    world.seed + 1,
                    &config,
                    registry(),
                    Arc::new(RiverField::empty()),
                )
                .is_none(),
                "a different seed must invalidate the cache"
            );
        }
    }

    #[test]
    fn incremental_base_loader_matches_one_shot() {
        let reg = registry();
        let total = (WORLD_SIZE_CHUNKS as usize) * (WORLD_SIZE_CHUNKS as usize);
        let mut chunks = vec![Vec::new(); total];
        chunks[5].push(grid_plant(10.0, 50.0, 0));
        chunks[5].push(grid_plant(20.0, 80.0, 3));
        chunks[42].push(grid_plant(33.0, 12.0, 2));
        chunks[total - 1].push(grid_plant(10.0, 50.0, 1));
        for c in &mut chunks {
            sort_chunk_canonical(c);
        }
        let world = test_world(chunks, Arc::clone(&reg));
        let config = GameConfig::default();
        let gen_key = 0x0011_2233_4455_6677u64;

        // Brotli decode is quality-agnostic, so both shipped qualities must drive
        // the incremental loader to the same world the one-shot path produces.
        for quality in [PlantWorld::LOCAL_QUALITY, PlantWorld::DOWNLOAD_QUALITY] {
            let bytes = world.serialize_base(gen_key, quality);
            let one_shot = PlantWorld::from_base_snapshot(
                &bytes,
                gen_key,
                world.seed,
                &config,
                Arc::clone(&reg),
                Arc::new(RiverField::empty()),
            )
            .expect("one-shot load");

            let mut loader = BaseLoader::new(
                &bytes,
                gen_key,
                world.seed,
                &config,
                Arc::clone(&reg),
                Arc::new(RiverField::empty()),
            )
            .expect("incremental loader header validates");
            let mut steps = 0;
            let incremental = loop {
                steps += 1;
                match loader.step().expect("incremental step") {
                    LoadStep::Working(frac) => assert!((0.0..=1.0).contains(&frac)),
                    LoadStep::Done(world) => break world,
                }
            };
            // A full-size world spans far more than one rebuild batch, so the load
            // must genuinely span several steps (that's the whole point — it yields).
            assert!(steps > 3, "expected a multi-step load, took {steps}");

            assert_eq!(incremental.population(), one_shot.population());
            assert_eq!(incremental.populated_chunks(), one_shot.populated_chunks());
            assert_eq!(incremental.chunks, one_shot.chunks);
            assert_eq!(incremental.biome, one_shot.biome);
            assert_eq!(incremental.cell_bytes, one_shot.cell_bytes);
            assert_eq!(incremental.base_count, one_shot.base_count);
            assert_eq!(incremental.immature, one_shot.immature);
        }

        // A mismatched generation key or seed is rejected at construction, exactly
        // like [`PlantWorld::from_base_snapshot`].
        let bytes = world.serialize_base(gen_key, PlantWorld::LOCAL_QUALITY);
        assert!(BaseLoader::new(
            &bytes,
            gen_key ^ 1,
            world.seed,
            &config,
            registry(),
            Arc::new(RiverField::empty()),
        )
        .is_err());
        assert!(BaseLoader::new(
            &bytes,
            gen_key,
            world.seed + 1,
            &config,
            registry(),
            Arc::new(RiverField::empty()),
        )
        .is_err());
    }

    #[test]
    fn apply_saved_spread_rejects_a_corrupt_plant_count() {
        // A blob whose declared plant count far exceeds its bytes must be rejected
        // (not reserve a huge allocation) — header + one chunk claiming 1e9 plants.
        let mut buf = Vec::new();
        buf.extend_from_slice(&SPREAD_MAGIC.to_le_bytes());
        buf.extend_from_slice(&SPREAD_VERSION.to_le_bytes());
        buf.extend_from_slice(&7u32.to_le_bytes()); // seed (matches test_world)
        buf.extend_from_slice(&1u32.to_le_bytes()); // one chunk
        buf.extend_from_slice(&0u32.to_le_bytes()); // chunk idx 0
        buf.extend_from_slice(&1_000_000_000u32.to_le_bytes()); // bogus count
        let storage = MemBytes::default();
        storage.save_bytes("plants", &buf).unwrap();

        let mut world = test_world(vec![vec![]], registry());
        assert_eq!(world.apply_saved_spread(&storage), 0);
        assert!(world.chunks[0].is_empty());
    }

    #[test]
    fn apply_saved_spread_rejects_a_mismatched_seed() {
        let reg = registry();
        let mut world = test_world(vec![vec![], vec![seedling(10.0, 0.0)]], Arc::clone(&reg));
        world.chunks[1].push(seedling(20.0, 24.0));
        world.immature[1] = 2;
        let storage = MemBytes::default();
        world.save_spread(&storage).unwrap();

        // A world with a different seed must ignore the save rather than corrupt.
        let mut other = test_world(vec![vec![], vec![seedling(10.0, 0.0)]], reg);
        other.seed = 999;
        assert_eq!(other.apply_saved_spread(&storage), 0);
        assert_eq!(other.chunks[1].len(), 1);
    }

    #[test]
    fn packed_plant_is_twenty_four_bytes() {
        assert_eq!(std::mem::size_of::<Plant>(), 24);
    }

    #[test]
    fn pack_round_trips_within_quantization_tolerance() {
        let origin_x = 5.0 * CHUNK_SIZE_METERS;
        let origin_z = 7.0 * CHUNK_SIZE_METERS;
        // Reconstruct against flat terrain so y is well-defined.
        let total = crate::world_core::chunk::CHUNK_GRID_RESOLUTION
            * crate::world_core::chunk::CHUNK_GRID_RESOLUTION;
        let terrain = ChunkTerrain {
            heights: vec![88.0; total],
            moisture: vec![0.5; total],
            river: vec![0.0; total],
            river_flow: vec![[0.0, 0.0]; total],
            river_surface: vec![88.0; total],
            min_height: 88.0,
            max_height: 88.0,
            has_water: false,
            has_river: false,
        };
        let original = PlantInstance {
            position: Vec3::new(origin_x + 123.4, 88.0, origin_z + 200.1),
            rotation: 2.0,
            height: 12.5,
            species_index: 3,
            growth_stage: GrowthStage::Mature,
            height_scale: 1.0,
            width_scale: 1.0,
            stress: 0.0,
            leaf_tint: [1.0, 1.0, 1.0, 1.0],
            decay: 0.0,
        };
        let packed = Plant::pack(
            &original,
            origin_x,
            origin_z,
            7,
            IVec2::new(5, 7),
            &terrain,
            0.0,
        );
        let back = packed.to_instance(IVec2::new(5, 7), &terrain);

        // Base plants are snapped to the storage grid in `pack`: 0.25 m per axis
        // (`POS_BITS`), 0.125 m height, ~1.4° rotation.
        assert!((back.position.x - original.position.x).abs() < 0.25);
        assert!((back.position.z - original.position.z).abs() < 0.25);
        assert!((back.position.y - 88.0).abs() < 1e-3);
        assert!((back.height - original.height).abs() < 0.13);
        assert!((back.rotation - original.rotation).abs() < 0.03);
        assert_eq!(back.species_index, original.species_index);
        assert_eq!(back.growth_stage, GrowthStage::Mature);
        assert_ne!(packed.genes, PlantGenes::default());
    }

    #[test]
    fn instances_rebase_to_the_raw_chunk_one_lap_away() {
        // A canonical chunk rendered a full world lap east lands one lap further
        // along x with its local offset preserved.
        let total = crate::world_core::chunk::CHUNK_GRID_RESOLUTION
            * crate::world_core::chunk::CHUNK_GRID_RESOLUTION;
        let terrain = ChunkTerrain {
            heights: vec![50.0; total],
            moisture: vec![0.5; total],
            river: vec![0.0; total],
            river_flow: vec![[0.0, 0.0]; total],
            river_surface: vec![50.0; total],
            min_height: 50.0,
            max_height: 50.0,
            has_water: false,
            has_river: false,
        };
        let plant = Plant {
            local_x: quantize_span(40.0),
            local_z: quantize_span(60.0),
            height: quantize_span(10.0),
            rotation: 0,
            species: 0,
            stage: MATURE,
            born_hour: 0.0,
            genes: PlantGenes::default(),
        };
        let canon = IVec2::new(3, 4);
        let raw = IVec2::new(3 + WORLD_SIZE_CHUNKS, 4);

        let here = plant.to_instance(canon, &terrain);
        let lap = plant.to_instance(raw, &terrain);

        let lap_meters = WORLD_SIZE_CHUNKS as f32 * CHUNK_SIZE_METERS;
        assert!((lap.position.x - (here.position.x + lap_meters)).abs() < 0.01);
        assert!((lap.position.z - here.position.z).abs() < 0.01);
        assert!((lap.position.y - here.position.y).abs() < 1e-3);
    }

    #[test]
    fn neighbourhood_saturation_requires_self_and_all_eight_neighbours() {
        let n = WORLD_SIZE_CHUNKS;
        let cells = (n * n) as usize;
        let saturate = |sat: &mut [bool], cx: i32, cz: i32| {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let nx = (cx + dx).rem_euclid(n);
                    let nz = (cz + dz).rem_euclid(n);
                    sat[(nz * n + nx) as usize] = true;
                }
            }
        };

        // Interior chunk with its full 3×3 block saturated.
        let mut sat = vec![false; cells];
        saturate(&mut sat, 5, 5);
        let idx = (5 * n + 5) as usize;
        assert!(neighbourhood_saturated(&sat, idx));

        // Clearing any one neighbour breaks it.
        sat[(6 * n + 5) as usize] = false;
        assert!(!neighbourhood_saturated(&sat, idx));

        // The centre itself must be saturated too.
        sat[(6 * n + 5) as usize] = true;
        sat[idx] = false;
        assert!(!neighbourhood_saturated(&sat, idx));

        // Corner chunk (0,0): neighbours wrap around the torus to row/col n-1.
        let mut wrap = vec![false; cells];
        saturate(&mut wrap, 0, 0);
        assert!(neighbourhood_saturated(&wrap, 0));
        wrap[((n - 1) * n + (n - 1)) as usize] = false; // the (-1,-1) diagonal
        assert!(!neighbourhood_saturated(&wrap, 0));
    }

    #[test]
    fn life_schedule_jitters_within_bounds_and_orders_thresholds() {
        let reg = registry();
        let species = &reg.species[0];
        let lifespan = species.placement.lifespan_hours as f64;
        let snag = species.placement.snag_hours as f64;
        assert!(lifespan > 0.0 && snag > 0.0);

        for i in 0..200u32 {
            let plant = seedling(i as f32, i as f32 * 7.0);
            let phenotype = evaluate_phenotype(plant.genes, species, neutral_env());
            let maturity_scale = phenotype.maturity_scale as f64;
            let expected_mature_at = (species.placement.seedling_hours
                + species.placement.young_hours) as f64
                * maturity_scale;
            let stress_life = (1.0 - phenotype.stress as f64 * 0.35).clamp(0.35, 1.0);
            let lifespan_scale = (phenotype.lifespan_scale as f64 * stress_life).clamp(0.25, 2.0);

            // Spread plants: phenotype-scaled growth, then ±25% lifespan jitter.
            let s1 = life_schedule(7, i as usize, &plant, species, false, neutral_env());
            assert!((s1.mature_at - expected_mature_at).abs() < 1e-6);
            let lived = s1.dead_at - s1.mature_at;
            assert!(
                lived >= lifespan * lifespan_scale * 0.75 - 1e-6
                    && lived <= lifespan * lifespan_scale * 1.25 + 1e-6
            );
            let stood = s1.gone_at - s1.dead_at;
            assert!(stood >= snag * 0.75 - 1e-6 && stood <= snag * 1.25 + 1e-6);

            // Base plants: a uniform fraction of one lifespan, so the starting
            // forest trickles out instead of dying in a wave.
            let s2 = life_schedule(7, i as usize, &plant, species, true, neutral_env());
            let lived = s2.dead_at - s2.mature_at;
            assert!(lived >= 0.0 && lived <= lifespan * lifespan_scale + 1e-6);
        }
    }

    #[test]
    fn life_schedule_scales_with_genes_and_environment() {
        let reg = registry();
        let species = &reg.species[0];
        let mut fast = seedling(20.0, 0.0);
        fast.genes = PlantGenes {
            wet_pref: 128,
            alt_pref: 128,
            stress_width: 200,
            capture: 220,
            fecundity: 30,
            seed_mass: 30,
            dispersal: 128,
            timing: 240,
        };
        let mut slow = fast;
        slow.genes.timing = 20;
        slow.genes.fecundity = 240;

        let env = neutral_env();
        let fast_schedule = life_schedule(7, 0, &fast, species, false, env);
        let slow_schedule = life_schedule(7, 0, &slow, species, false, env);
        assert!(fast_schedule.mature_at < slow_schedule.mature_at);
        assert!(
            fast_schedule.dead_at - fast_schedule.mature_at
                > slow_schedule.dead_at - slow_schedule.mature_at
        );

        let harsh_env = PlantEnvironment {
            moisture: 0.0,
            altitude: 1.0,
            ..env
        };
        let harsh = life_schedule(7, 0, &fast, species, false, harsh_env);
        assert!(harsh.dead_at - harsh.mature_at < fast_schedule.dead_at - fast_schedule.mature_at);
    }

    #[test]
    fn snags_stand_then_despawn_freeing_the_ground() {
        let reg = registry();
        let total = (WORLD_SIZE_CHUNKS as usize) * (WORLD_SIZE_CHUNKS as usize);
        let n = WORLD_SIZE_CHUNKS as usize;
        let idx = 5 * n + 5;
        let mut chunks = vec![Vec::new(); total];
        chunks[idx].push(grid_plant(10.0, 50.0, 0)); // base plant, mature
        let mut world = test_world(chunks, Arc::clone(&reg));
        for flag in world.saturated.iter_mut() {
            *flag = true;
        }

        let plant = world.chunks[idx][0];
        let schedule = life_schedule(
            world.seed,
            idx,
            &plant,
            &reg.species[0],
            true,
            plant_life_environment(
                &world.heightmap,
                &world.rivers,
                world.sea_level,
                idx,
                &plant,
            ),
        );
        assert!(schedule.gone_at.is_finite());

        // At dead_at the plant flips to a standing snag: still present, not
        // immature, and no longer a spread source.
        assert!(world.tick_growth(plant.born_hour as f64 + schedule.dead_at + 0.01));
        assert_eq!(world.chunks[idx][0].stage, DEAD);
        assert_eq!(world.population(), 1);
        assert_eq!(world.immature[idx], 0);
        let emit_ctx = EmitContext {
            registry: &reg,
            heightmap: &world.heightmap,
            rivers: &world.rivers,
            sea_level: world.sea_level,
            seed: world.seed,
            bucket: 0,
        };
        assert!(emit_chunk_candidates(idx, &world.chunks, &emit_ctx).is_empty());

        // At gone_at it despawns: record removed, counters adjusted, and the
        // chunk plus its 8 neighbours woken so spread can reclaim the ground.
        assert!(world.tick_growth(plant.born_hour as f64 + schedule.gone_at + 0.01));
        assert!(world.chunks[idx].is_empty());
        assert_eq!(world.population(), 0);
        assert_eq!(world.base_count[idx], 0);
        assert_eq!(world.populated_chunks(), 0);
        let woken = world.saturated.iter().filter(|s| !**s).count();
        assert_eq!(woken, 9, "the 3x3 neighbourhood must be unsaturated");
        for dz in -1i32..=1 {
            for dx in -1i32..=1 {
                let nidx = ((5 + dz) * n as i32 + (5 + dx)) as usize;
                assert!(!world.saturated[nidx]);
            }
        }
    }

    #[test]
    fn immortal_species_never_die() {
        let reg = registry();
        let cattail = reg
            .species
            .iter()
            .position(|s| s.placement.lifespan_hours <= 0.0)
            .expect("an immortal species (cattail)") as u8;
        let mut world = test_world(
            vec![vec![Plant {
                species: cattail,
                ..grid_plant(10.0, 50.0, 0)
            }]],
            Arc::clone(&reg),
        );

        // Far beyond any mortal lifespan, the reed clump still stands mature.
        world.tick_growth(1.0e9);
        assert_eq!(world.chunks[0].len(), 1);
        assert_eq!(world.chunks[0][0].stage, MATURE);
        assert_eq!(world.population(), 1);
    }

    #[test]
    fn a_reloaded_world_reaps_exactly_what_a_live_world_did() {
        let reg = registry();
        // One mortal base plant plus two spread seedlings born at hour 0.
        let base = grid_plant(10.0, 50.0, 0);
        let make = |reg: Arc<PlantRegistry>| {
            let mut world = test_world(vec![Vec::new(), vec![base], Vec::new()], reg);
            world.chunks[1].push(seedling(20.0, 0.0));
            world.chunks[1].push(seedling(30.0, 0.0));
            world.immature[1] = 2;
            world.population += 2;
            world
        };

        let mut live = make(Arc::clone(&reg));
        // Save before anything dies, so the blob still holds every plant.
        let storage = MemBytes::default();
        live.save_spread(&storage).unwrap();

        // Pick the first despawn among the three plants and tick just past it:
        // the live world reaps that plant (and only it).
        let species = &reg.species[0];
        let gone = |pi: usize, is_base: bool| {
            let plant = live.chunks[1][pi];
            let s = life_schedule(
                live.seed,
                1,
                &plant,
                species,
                is_base,
                plant_life_environment(&live.heightmap, &live.rivers, live.sea_level, 1, &plant),
            );
            plant.born_hour as f64 + s.gone_at
        };
        let t = gone(0, true).min(gone(1, false)).min(gone(2, false)) + 0.01;
        live.tick_growth(t);
        assert!(
            live.chunks[1].len() < 3,
            "at least one plant must have despawned"
        );

        // A fresh world restored from the pre-death save and ticked to the same
        // hour converges on the identical plant list and derived counters.
        let mut reloaded = test_world(vec![Vec::new(), vec![base], Vec::new()], reg);
        reloaded.apply_saved_spread(&storage);
        reloaded.tick_growth(t);

        assert_eq!(reloaded.chunks[1], live.chunks[1]);
        assert_eq!(reloaded.population(), live.population());
        assert_eq!(reloaded.base_count[1], live.base_count[1]);
        assert_eq!(reloaded.immature[1], live.immature[1]);
    }

    #[test]
    fn tick_growth_promotes_immature_plants_to_mature() {
        let reg = registry();
        let mut world = test_world(
            vec![vec![Plant {
                local_x: 0,
                local_z: 0,
                height: quantize_span(5.0),
                rotation: 0,
                species: 0,
                stage: stage_to_u8(GrowthStage::Seedling),
                born_hour: 0.0,
                genes: PlantGenes::default(),
            }]],
            Arc::clone(&reg),
        );
        // The seedling is a spread plant, not part of the mature-from-creation
        // base prefix.
        world.base_count[0] = 0;
        let plant = world.chunks[0][0];
        let mature_at = life_schedule(
            world.seed,
            0,
            &plant,
            &reg.species[0],
            false,
            plant_life_environment(&world.heightmap, &world.rivers, world.sea_level, 0, &plant),
        )
        .mature_at;

        // Before any growth time, it stays a seedling.
        assert!(!world.tick_growth(0.0));
        assert_eq!(world.chunks[0][0].stage, stage_to_u8(GrowthStage::Seedling));
        assert_eq!(world.immature[0], 1);

        // Well past maturity, it becomes mature and the chunk drops to zero
        // immature — so later ticks skip it.
        assert!(world.tick_growth(mature_at + 1.0));
        assert_eq!(world.chunks[0][0].stage, MATURE);
        assert_eq!(world.immature[0], 0);
    }

    // Find a canonical chunk + local position suitable for `species_idx` so the
    // spread-landing test exercises the real terrain validation deterministically.
    fn find_suitable_spot(world: &PlantWorld, species_idx: usize) -> (usize, f32, f32) {
        let placement = &world.registry.species[species_idx].placement;
        let n = WORLD_SIZE_CHUNKS;
        for cz in 0..n {
            for cx in 0..n {
                // Sample the chunk centre.
                let wx = cx as f32 * CHUNK_SIZE_METERS + 128.0;
                let wz = cz as f32 * CHUNK_SIZE_METERS + 128.0;
                let height = world.heightmap.sample_height(wx, wz);
                let moisture = world.heightmap.sample_moisture(wx, wz);
                if height < world.sea_level
                    || height < placement.min_altitude
                    || height > placement.max_altitude
                    || moisture < placement.min_moisture
                    || moisture > placement.max_moisture
                    || slope_at(&world.heightmap, wx, wz) > placement.max_slope
                {
                    continue;
                }
                let biome = classify(height, moisture, &world.biome_config);
                if placement.biomes.iter().any(|b| b == biome_name(biome)) {
                    return ((cz * n + cx) as usize, 128.0, 128.0);
                }
            }
        }
        panic!("no suitable spot found for species {species_idx}");
    }

    fn site_genes(world: &PlantWorld, world_x: f32, world_z: f32) -> PlantGenes {
        let height = world.heightmap.sample_height(world_x, world_z);
        let moisture = world.heightmap.sample_moisture(world_x, world_z);
        PlantGenes {
            wet_pref: (moisture.clamp(0.0, 1.0) * u8::MAX as f32).round() as u8,
            alt_pref: (normalize_altitude(height, world.sea_level) * u8::MAX as f32).round() as u8,
            stress_width: 230,
            capture: 128,
            fecundity: 20,
            seed_mass: 240,
            dispersal: 20,
            timing: 128,
        }
    }

    #[test]
    fn spread_landing_enforces_species_spacing() {
        // Many candidates crowded onto one suitable spot: spacing admits only one.
        let reg = registry();
        let world = test_world(vec![Vec::new(); 4], Arc::clone(&reg));
        let (chunk_idx, lx, lz) = find_suitable_spot(&world, 0);
        let cx = (chunk_idx as i32) % WORLD_SIZE_CHUNKS;
        let cz = (chunk_idx as i32) / WORLD_SIZE_CHUNKS;
        let wx = cx as f32 * CHUNK_SIZE_METERS + lx;
        let wz = cz as f32 * CHUNK_SIZE_METERS + lz;
        let good_genes = site_genes(&world, wx, wz);

        let ctx = SpreadContext {
            seed: world.seed,
            heightmap: &world.heightmap,
            rivers: &world.rivers,
            registry: &reg,
            biome_config: &world.biome_config,
            houses: &world.houses,
            sea_level: world.sea_level,
            born_hour: 0.0,
        };
        let mk = |order: u64| SpreadCandidate {
            target: chunk_idx as u32,
            local_x: lx,
            local_z: lz,
            world_x: wx,
            world_z: wz,
            height: 6.0,
            rotation: 0.0,
            species: 0,
            genes: PlantGenes::from_bytes([
                good_genes.wet_pref,
                good_genes.alt_pref,
                230,
                128,
                20,
                240,
                20,
                order as u8,
            ]),
            order,
        };
        let mut plants = Vec::new();
        let mut crowd = vec![mk(0), mk(1), mk(2), mk(3), mk(4)];
        let accepted = land_chunk_candidates(&mut plants, &mut crowd, &ctx);
        assert_eq!(accepted, 1, "spacing must reject coincident candidates");
        assert_eq!(plants.len(), 1);
        assert_eq!(plants[0].genes.wet_pref, good_genes.wet_pref);
        assert_eq!(plants[0].genes.alt_pref, good_genes.alt_pref);

        // A second pass at the very same spot adds nothing — the gate is stable,
        // which is what makes the global spread terminate.
        let mut again = vec![mk(5)];
        assert_eq!(land_chunk_candidates(&mut plants, &mut again, &ctx), 0);
        assert_eq!(plants.len(), 1);
    }

    #[test]
    fn competition_pressure_is_stronger_for_similar_neighbours() {
        let similar = PlantGenes {
            wet_pref: 120,
            capture: 220,
            ..PlantGenes::default()
        };
        let dissimilar = PlantGenes {
            wet_pref: 240,
            capture: 220,
            ..PlantGenes::default()
        };
        let candidate = PlantGenes {
            wet_pref: 118,
            ..PlantGenes::default()
        };

        let mut similar_grid = CompetitionGrid {
            cells: vec![Vec::new(); (SPACING_GRID_SIDE * SPACING_GRID_SIDE) as usize],
        };
        similar_grid.insert(128.0, 128.0, 12.0, MATURE, similar);
        let mut dissimilar_grid = CompetitionGrid {
            cells: vec![Vec::new(); (SPACING_GRID_SIDE * SPACING_GRID_SIDE) as usize],
        };
        dissimilar_grid.insert(128.0, 128.0, 12.0, MATURE, dissimilar);

        let (_, similar_root) = similar_grid.pressure(136.0, 128.0, candidate);
        let (_, dissimilar_root) = dissimilar_grid.pressure(136.0, 128.0, candidate);
        assert!(similar_root > dissimilar_root);
    }

    #[test]
    fn competition_pressure_reduces_establishment() {
        let species = &registry().species[0];
        let genes = PlantGenes {
            wet_pref: 128,
            alt_pref: 128,
            stress_width: 200,
            seed_mass: 200,
            dispersal: 80,
            fecundity: 80,
            ..PlantGenes::default()
        };
        let open = evaluate_phenotype(
            genes,
            species,
            PlantEnvironment {
                moisture: 0.5,
                altitude: 0.5,
                river_wetness: 0.0,
                shade: 0.0,
                root_pressure: 0.0,
            },
        );
        let crowded = evaluate_phenotype(
            genes,
            species,
            PlantEnvironment {
                moisture: 0.5,
                altitude: 0.5,
                river_wetness: 0.0,
                shade: 0.6,
                root_pressure: 0.6,
            },
        );
        assert!(open.establishment_chance > crowded.establishment_chance);
    }

    #[test]
    fn evolution_region_inspector_reports_gene_and_stage_summaries() {
        let reg = registry();
        let total = (WORLD_SIZE_CHUNKS as usize) * (WORLD_SIZE_CHUNKS as usize);
        let mut chunks = vec![Vec::new(); total];
        chunks[0].push(grid_plant(40.0, 40.0, 0));
        chunks[0].push(Plant {
            stage: stage_to_u8(GrowthStage::Seedling),
            genes: PlantGenes::from_bytes([200, 100, 50, 25, 75, 125, 175, 225]),
            ..grid_plant(46.0, 40.0, 0)
        });
        let world = test_world(chunks, reg);
        let report = world.inspect_evolution_region(42.0, 40.0, 16.0);

        assert_eq!(report["plant_count"].as_u64(), Some(2));
        assert!(report["genes"]["wet_pref"]["mean"].as_f64().unwrap() > 0.0);
        assert!(report["phenotype"]["abiotic_fitness"].as_f64().unwrap() >= 0.0);
        assert_eq!(report["stages"]["mature"].as_u64(), Some(1));
        assert_eq!(report["stages"]["seedling"].as_u64(), Some(1));
        assert_eq!(report["top_species"][0]["count"].as_u64(), Some(2));
    }

    #[test]
    fn spread_landing_keeps_seedlings_clear_of_houses() {
        // A seedling that lands on top of a house is rejected; the same seedling
        // just beyond the species' clearance is accepted.
        let reg = registry();
        let mut world = test_world(vec![Vec::new(); 4], Arc::clone(&reg));
        // `find_suitable_spot` returns a canonical-grid index, so size the house
        // index to the full grid (as a real world does) before indexing into it.
        world.houses = vec![Vec::new(); (WORLD_SIZE_CHUNKS * WORLD_SIZE_CHUNKS) as usize];
        let (chunk_idx, lx, lz) = find_suitable_spot(&world, 0);
        let cx = (chunk_idx as i32) % WORLD_SIZE_CHUNKS;
        let cz = (chunk_idx as i32) / WORLD_SIZE_CHUNKS;
        let wx = cx as f32 * CHUNK_SIZE_METERS + lx;
        let wz = cz as f32 * CHUNK_SIZE_METERS + lz;
        let clearance = house_clearance(&reg.species[0].kind);
        let good_genes = site_genes(&world, wx, wz);

        let candidate =
            |target: usize, world_x: f32, world_z: f32, lx: f32, lz: f32| SpreadCandidate {
                target: target as u32,
                local_x: lx,
                local_z: lz,
                world_x,
                world_z,
                height: 6.0,
                rotation: 0.0,
                species: 0,
                genes: good_genes,
                order: 0,
            };

        // A house sitting on the candidate must reject it.
        world.houses[chunk_idx] = vec![Vec2::new(wx, wz)];
        let mut plants = Vec::new();
        let mut blocked = vec![candidate(chunk_idx, wx, wz, lx, lz)];
        let accepted = land_chunk_candidates(
            &mut plants,
            &mut blocked,
            &SpreadContext {
                seed: world.seed,
                heightmap: &world.heightmap,
                rivers: &world.rivers,
                registry: &reg,
                biome_config: &world.biome_config,
                houses: &world.houses,
                sea_level: world.sea_level,
                born_hour: 0.0,
            },
        );
        assert_eq!(accepted, 0, "a seedling on top of a house must be rejected");
        assert!(plants.is_empty());

        // Move the house just past the clearance ring: the seedling is now allowed.
        world.houses[chunk_idx] = vec![Vec2::new(wx + clearance + 1.0, wz)];
        let mut plants = Vec::new();
        let mut allowed = vec![candidate(chunk_idx, wx, wz, lx, lz)];
        let accepted = land_chunk_candidates(
            &mut plants,
            &mut allowed,
            &SpreadContext {
                seed: world.seed,
                heightmap: &world.heightmap,
                rivers: &world.rivers,
                registry: &reg,
                biome_config: &world.biome_config,
                houses: &world.houses,
                sea_level: world.sea_level,
                born_hour: 0.0,
            },
        );
        assert_eq!(
            accepted, 1,
            "a seedling beyond the clearance must be accepted"
        );
        assert_eq!(plants.len(), 1);
        assert_eq!(plants[0].genes, good_genes);
    }

    #[test]
    fn spread_emits_seedlings_for_mature_plants() {
        // A mature plant whose spread roll succeeds emits at least one candidate
        // anchored near it; immature plants emit none.
        let reg = registry();
        let n = WORLD_SIZE_CHUNKS;
        let species0 = &reg.species[0];
        let probe = test_world(vec![Vec::new(); (n * n) as usize], Arc::clone(&reg));
        let mut found = None;
        for z in 0..n {
            for x in 0..n {
                let canon = IVec2::new(x, z);
                let idx = (canon.y * n + canon.x) as usize;
                let src_x = canon.x as f32 * CHUNK_SIZE_METERS + 128.0;
                let src_z = canon.y as f32 * CHUNK_SIZE_METERS + 128.0;
                let mut chunks = vec![Vec::new(); (n * n) as usize];
                chunks[idx].push(Plant {
                    local_x: quantize_span(128.0),
                    local_z: quantize_span(128.0),
                    height: quantize_span(10.0),
                    rotation: 0,
                    species: 0,
                    stage: MATURE,
                    born_hour: 0.0,
                    genes: PlantGenes {
                        fecundity: 240,
                        ..site_genes(&probe, src_x, src_z)
                    },
                });
                let emit_ctx = EmitContext {
                    registry: &reg,
                    heightmap: &probe.heightmap,
                    rivers: &probe.rivers,
                    sea_level: probe.sea_level,
                    seed: 7,
                    bucket: 0,
                };
                let cands = emit_chunk_candidates(idx, &chunks, &emit_ctx);
                if !cands.is_empty() {
                    found = Some((canon, idx, chunks, cands));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let (canon, idx, mut chunks, mature) =
            found.expect("a chunk whose phenotype-scaled plant 0 spreads");
        assert!(!mature.is_empty(), "a firing mature plant emits candidates");
        let emit_ctx = EmitContext {
            registry: &reg,
            heightmap: &probe.heightmap,
            rivers: &probe.rivers,
            sea_level: probe.sea_level,
            seed: 7,
            bucket: 0,
        };
        let again = emit_chunk_candidates(idx, &chunks, &emit_ctx);
        assert_eq!(
            mature.iter().map(|c| c.genes).collect::<Vec<_>>(),
            again.iter().map(|c| c.genes).collect::<Vec<_>>()
        );
        let parent_pheno = evaluate_phenotype(
            chunks[idx][0].genes,
            species0,
            plant_environment(
                &probe.heightmap,
                &probe.rivers,
                probe.sea_level,
                canon.x as f32 * CHUNK_SIZE_METERS + 128.0,
                canon.y as f32 * CHUNK_SIZE_METERS + 128.0,
            ),
        );
        for c in &mature {
            // Candidate world position is within spread_radius of the source.
            let dx = c.world_x - (canon.x as f32 * CHUNK_SIZE_METERS + 128.0);
            let dz = c.world_z - (canon.y as f32 * CHUNK_SIZE_METERS + 128.0);
            assert!(
                (dx * dx + dz * dz).sqrt()
                    <= species0.placement.spread_radius * parent_pheno.spread_radius_scale + 0.01
            );
            for (child, parent) in c
                .genes
                .to_bytes()
                .into_iter()
                .zip(chunks[idx][0].genes.to_bytes())
            {
                let delta = child.abs_diff(parent);
                assert!(delta == 0 || delta <= 10);
            }
        }

        // The same plant as a seedling emits nothing.
        chunks[idx][0].stage = stage_to_u8(GrowthStage::Seedling);
        assert!(emit_chunk_candidates(idx, &chunks, &emit_ctx).is_empty());
    }

    // Replays the runtime's growth+spread loop at a coarse per-frame time step
    // (mimicking a high `day_speed`, where a frame spans many sim-days) and
    // asserts the mortal forest survives. Before the day-bucketed roll + catch-up
    // spread, reproduction was throttled to one identical pass per frame while
    // death stayed analytic, so a forest like this went extinct.
    #[test]
    fn forest_survives_high_dayspeed_fast_forward() {
        let reg = registry();
        let n = WORLD_SIZE_CHUNKS;
        // Sanity: species 0 must be a mortal that can spread, or the test proves
        // nothing about reproduction keeping pace with death.
        let p0 = &reg.species[0].placement;
        assert!(p0.lifespan_hours > 0.0, "species 0 must be mortal");
        assert!(p0.spread_chance > 0.0, "species 0 must reproduce");
        // Derive the run length from the configured lifespan so rebalancing the
        // species can't silently shorten the test below a meaningful turnover.
        let lifespan = p0.lifespan_hours as f64;

        // Seed one suitable chunk with a dense mature base forest.
        let mut chunks: Vec<Vec<Plant>> = (0..(n * n)).map(|_| Vec::new()).collect();
        let probe = test_world(vec![Vec::new(); (n * n) as usize], Arc::clone(&reg));
        let (chunk_idx, _, _) = find_suitable_spot(&probe, 0);
        let cx = (chunk_idx as i32) % WORLD_SIZE_CHUNKS;
        let cz = (chunk_idx as i32) / WORLD_SIZE_CHUNKS;
        let mut x = 8.0f32;
        while x < 248.0 {
            let mut z = 8.0f32;
            while z < 248.0 {
                let world_x = cx as f32 * CHUNK_SIZE_METERS + x;
                let world_z = cz as f32 * CHUNK_SIZE_METERS + z;
                chunks[chunk_idx].push(Plant {
                    local_x: quantize_span(x),
                    local_z: quantize_span(z),
                    height: quantize_span(6.0),
                    rotation: 0,
                    species: 0,
                    stage: MATURE,
                    born_hour: 0.0,
                    genes: PlantGenes {
                        fecundity: 220,
                        seed_mass: 150,
                        dispersal: 190,
                        ..site_genes(&probe, world_x, world_z)
                    },
                });
                z += 12.0;
            }
            x += 12.0;
        }
        let seeded = chunks[chunk_idx].len();
        assert!(seeded > 100, "need a dense starting forest, got {seeded}");
        let mut world = test_world(chunks, reg);

        // One "frame" jumps 240 sim-hours (10 sim-days). With the old code this
        // bottomed out near extinction; with catch-up spread it must persist.
        const FRAME_JUMP: f64 = 240.0;
        let mut t = 0.0f64;
        let mut last_spread = -SPREAD_TICK_HOURS;
        // Run well past several full lifespans so the base cohort is long gone and
        // only spread-born descendants remain.
        while t <= 6.0 * lifespan {
            world.tick_growth(t);
            // Mirror the runtime's catch-up spread loop.
            let mut rounds = 0;
            while t - last_spread >= SPREAD_TICK_HOURS && rounds < 8 {
                last_spread += SPREAD_TICK_HOURS;
                world.tick_spread(last_spread);
                rounds += 1;
            }
            if t - last_spread >= SPREAD_TICK_HOURS {
                last_spread = t;
            }
            t += FRAME_JUMP;
        }

        // Survival, not a specific count: the population must stay healthily above
        // zero rather than collapsing to the immortal residue.
        assert!(
            world.population() >= seeded / 2,
            "forest collapsed under high day_speed: {} survivors from {seeded} (expected >= {})",
            world.population(),
            seeded / 2,
        );
    }
}
