//! Resident, whole-world plant store.
//!
//! Iteration 1 of the live simulation keeps every plant in every one of the
//! `WORLD_SIZE_CHUNKS²` canonical chunks resident and simulates them on one
//! global clock — see [`docs/LIVE_SIM_ITERATION_1.md`]. This module is the
//! store: a packed `Plant` per canonical chunk, generated once at world
//! creation, read by the render bridge, and ticked for growth + spread on the
//! global clock. Persistence (M4/M5) saves it.

use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

use glam::{IVec2, Vec3};

use crate::world_core::biome::{classify, Biome};
use crate::world_core::chunk::{
    canonical_chunk, ChunkTerrain, PlantInstance, CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS,
    WORLD_SIZE_METERS,
};
use crate::world_core::chunk_generator::ChunkGenerator;
use crate::world_core::config::{BiomeConfig, GameConfig};
use crate::world_core::content::sampling::{hash4, hash_to_unit_float};
use crate::world_core::heightmap::Heightmap;
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::lifecycle::GrowthStage;

/// Packed per-plant record — **16 bytes** (`repr(C)`), validated against the
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
}

const MATURE: u8 = stage_to_u8(GrowthStage::Mature);

const fn stage_to_u8(stage: GrowthStage) -> u8 {
    match stage {
        GrowthStage::Seedling => 0,
        GrowthStage::Young => 1,
        GrowthStage::Mature => 2,
    }
}

fn stage_from_u8(value: u8) -> GrowthStage {
    match value {
        0 => GrowthStage::Seedling,
        1 => GrowthStage::Young,
        _ => GrowthStage::Mature,
    }
}

// Quantize `[0, CHUNK_SIZE_METERS)` onto the `u16` range using a 2^16 divisor and
// `floor`, so the dequantized value is always strictly less than
// `CHUNK_SIZE_METERS`. A plant therefore never reconstructs exactly on the chunk
// boundary, where `floor(pos / CHUNK_SIZE_METERS)` would attribute it to the
// neighbouring chunk.
const SPAN_STEPS: f32 = 65536.0;

fn quantize_span(value: f32) -> u16 {
    (value / CHUNK_SIZE_METERS * SPAN_STEPS)
        .floor()
        .clamp(0.0, u16::MAX as f32) as u16
}

fn dequantize_span(value: u16) -> f32 {
    value as f32 / SPAN_STEPS * CHUNK_SIZE_METERS
}

fn quantize_rotation(radians: f32) -> u16 {
    (radians.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * u16::MAX as f32)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16
}

impl Plant {
    /// Pack a generated plant whose absolute position lies in the chunk whose
    /// world origin is `(origin_x, origin_z)`.
    fn pack(plant: &PlantInstance, origin_x: f32, origin_z: f32) -> Self {
        Plant {
            local_x: quantize_span(plant.position.x - origin_x),
            local_z: quantize_span(plant.position.z - origin_z),
            height: quantize_span(plant.height),
            rotation: quantize_rotation(plant.rotation),
            species: plant.species_index as u8,
            stage: stage_to_u8(plant.growth_stage),
            born_hour: 0.0,
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
        }
    }
}

/// Per-canonical-chunk packed plant lists for the whole finite world.
pub struct PlantWorld {
    /// One list per canonical chunk, indexed `cz * WORLD_SIZE_CHUNKS + cx`.
    chunks: Vec<Vec<Plant>>,
    /// Count of plants not yet `Mature`, per chunk — lets the growth tick skip
    /// fully-grown chunks.
    immature: Vec<u32>,
    /// Per chunk: all-mature and saturated — the last spread pass added nothing
    /// here. A chunk's plants can only spread into the 8 adjacent chunks (spread
    /// radius ≪ chunk size), so phase 1 skips a chunk only when it *and* all eight
    /// neighbours are saturated. That keeps a packed chunk feeding a still-filling
    /// neighbour, while making the global pass cheap once a whole region fills.
    saturated: Vec<bool>,
    registry: Arc<PlantRegistry>,
    /// Terrain + rules for validating spread landings without the full per-chunk
    /// grid: the heightmap is point-sampled at each seedling position.
    heightmap: Heightmap,
    biome_config: BiomeConfig,
    sea_level: f32,
    seed: u32,
    /// Cached totals so `stats()` (called every frame) stays O(1), maintained as
    /// spread adds plants and transitions chunks empty→non-empty.
    population: usize,
    populated_chunks: usize,
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
        threads: usize,
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
        let generator = ChunkGenerator::new(seed, config, Arc::clone(&registry));

        // Pack one canonical chunk's base flora; terrain/biome are transient.
        let build_one = |idx: usize| -> Vec<Plant> {
            let cx = (idx as i32) % n;
            let cz = (idx as i32) / n;
            let coord = IVec2::new(cx, cz);
            let data = generator.generate_chunk(coord);
            let origin_x = cx as f32 * CHUNK_SIZE_METERS;
            let origin_z = cz as f32 * CHUNK_SIZE_METERS;
            let mut plants = Vec::with_capacity(data.content.base_plants.len());
            for plant in &data.content.base_plants {
                plants.push(Plant::pack(plant, origin_x, origin_z));
            }
            plants.shrink_to_fit();
            plants
        };

        #[cfg(not(target_arch = "wasm32"))]
        let chunks: Vec<Vec<Plant>> = {
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
        let chunks: Vec<Vec<Plant>> = {
            let _ = threads;
            (0..total).map(build_one).collect()
        };

        let population = chunks.iter().map(Vec::len).sum();
        let populated_chunks = chunks.iter().filter(|c| !c.is_empty()).count();
        let immature = chunks
            .iter()
            .map(|c| c.iter().filter(|p| p.stage != MATURE).count() as u32)
            .collect();

        let saturated = vec![false; chunks.len()];
        Self {
            chunks,
            immature,
            saturated,
            registry,
            heightmap: Heightmap::new(seed, config.heightmap.clone()),
            biome_config: config.biome.clone(),
            sea_level: config.sea_level,
            seed,
            population,
            populated_chunks,
        }
    }

    /// Total plants across the whole world (loaded or not). O(1).
    pub fn population(&self) -> usize {
        self.population
    }

    /// Number of canonical chunks holding at least one plant. O(1).
    pub fn populated_chunks(&self) -> usize {
        self.populated_chunks
    }

    /// Packed plants for a canonical chunk.
    fn chunk(&self, canon: IVec2) -> &[Plant] {
        &self.chunks[chunk_index(canon)]
    }

    /// Reconstruct renderable instances for the raw chunk at `raw_coord`, reading
    /// the canonical chunk's plant list and resampling ground height from
    /// `terrain`.
    pub fn instances_for(&self, raw_coord: IVec2, terrain: &ChunkTerrain) -> Vec<PlantInstance> {
        let canon = canonical_chunk(raw_coord);
        self.chunk(canon)
            .iter()
            .map(|plant| plant.to_instance(raw_coord, terrain))
            .collect()
    }

    /// Advance growth on the global clock. Growth is analytic — each plant's
    /// stage is a function of its `born_hour` and the species' stage durations —
    /// so this only walks chunks that still hold immature plants. Returns whether
    /// any plant's stage advanced (so the render bridge can refresh loaded chunks).
    pub fn tick_growth(&mut self, total_hours: f64) -> bool {
        let registry = &self.registry;
        let mut changed = false;
        for (idx, chunk) in self.chunks.iter_mut().enumerate() {
            if self.immature[idx] == 0 {
                continue;
            }
            let mut immature = 0u32;
            for plant in chunk.iter_mut() {
                if plant.stage != MATURE {
                    let next = stage_to_u8(stage_for(plant, total_hours, registry));
                    if next != plant.stage {
                        plant.stage = next;
                        changed = true;
                    }
                    if plant.stage != MATURE {
                        immature += 1;
                    }
                }
            }
            self.immature[idx] = immature;
        }
        changed
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

        // Phase 1: emit candidates. Read-only over the whole grid. A chunk is
        // skipped only when it and all eight neighbours are saturated, so a packed
        // chunk still seeds a not-yet-full neighbour across the border.
        let saturated = &self.saturated;
        let emit = |idx: usize| {
            if neighbourhood_saturated(saturated, idx) {
                return Vec::new();
            }
            emit_chunk_candidates(idx, &self.chunks, &self.registry, self.seed)
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
            heightmap: &self.heightmap,
            registry: &self.registry,
            biome_config: &self.biome_config,
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
        let mut any = false;
        for (idx, &count) in accepted.iter().enumerate() {
            if count > 0 {
                any = true;
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
            } else {
                // Nothing landed: saturated iff nothing is left growing here. A
                // neighbour maturing and spreading in (above) clears this again.
                self.saturated[idx] = self.immature[idx] == 0;
            }
        }
        any
    }
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
    /// Deterministic ordering key (source chunk, plant, seed) so landing order
    /// — and thus which candidates win spacing — is independent of parallelism.
    order: u64,
}

struct SpreadContext<'a> {
    heightmap: &'a Heightmap,
    registry: &'a PlantRegistry,
    biome_config: &'a BiomeConfig,
    sea_level: f32,
    born_hour: f32,
}

fn emit_chunk_candidates(
    idx: usize,
    chunks: &[Vec<Plant>],
    registry: &PlantRegistry,
    seed: u32,
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
        let Some(species) = registry.species.get(plant.species as usize) else {
            continue;
        };
        if spread_roll(seed, canon, pi as u32) >= species.placement.spread_chance.clamp(0.0, 1.0) {
            continue;
        }

        let src_x = cx as f32 * CHUNK_SIZE_METERS + dequantize_span(plant.local_x);
        let src_z = cz as f32 * CHUNK_SIZE_METERS + dequantize_span(plant.local_z);
        let [h_lo, h_hi] = species.height_range;
        let count = spread_seed_count(seed, canon, pi as u32);
        for seed_i in 0..count {
            let sub = (pi as u32).wrapping_mul(31).wrapping_add(seed_i);
            let angle = hash_unit(seed.wrapping_add(4101), canon, sub) * std::f32::consts::TAU;
            let distance = hash_unit(seed.wrapping_add(4102), canon, sub).sqrt()
                * species.placement.spread_radius.max(0.0);
            let height = h_lo + hash_unit(seed.wrapping_add(4201), canon, sub) * (h_hi - h_lo);
            let rotation = hash_unit(seed.wrapping_add(4202), canon, sub) * std::f32::consts::TAU;

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

        if slope_at(ctx.heightmap, candidate.world_x, candidate.world_z) > placement.max_slope {
            continue;
        }

        grid.insert(candidate.local_x, candidate.local_z);
        plants.push(Plant {
            local_x: quantize_span(candidate.local_x),
            local_z: quantize_span(candidate.local_z),
            height: quantize_span(candidate.height),
            rotation: quantize_rotation(candidate.rotation),
            species: candidate.species,
            stage: stage_to_u8(GrowthStage::Seedling),
            born_hour: ctx.born_hour,
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

fn spread_roll(seed: u32, canon: IVec2, plant_index: u32) -> f32 {
    hash_unit(seed.wrapping_add(4001), canon, plant_index)
}

fn spread_seed_count(seed: u32, canon: IVec2, plant_index: u32) -> u32 {
    1 + (hash_unit(seed.wrapping_add(4002), canon, plant_index) * 2.0).floor() as u32
}

fn hash_unit(seed: u32, canon: IVec2, sub: u32) -> f32 {
    hash_to_unit_float(hash4(seed, canon.x as u32, canon.y as u32, sub))
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

/// Analytic growth stage for a packed plant at `total_hours`.
fn stage_for(plant: &Plant, total_hours: f64, registry: &PlantRegistry) -> GrowthStage {
    let Some(species) = registry.species.get(plant.species as usize) else {
        return stage_from_u8(plant.stage);
    };
    let age = (total_hours - plant.born_hour as f64).max(0.0);
    let young_at = species.placement.seedling_hours.max(0.0) as f64;
    let mature_at = young_at + species.placement.young_hours.max(0.0) as f64;
    if age >= mature_at {
        GrowthStage::Mature
    } else if age >= young_at {
        GrowthStage::Young
    } else {
        GrowthStage::Seedling
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_core::herbarium::Herbarium;

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
            .map(|c| c.iter().filter(|p| p.stage != MATURE).count() as u32)
            .collect();
        let saturated = vec![false; chunks.len()];
        PlantWorld {
            chunks,
            immature,
            saturated,
            registry: reg,
            heightmap: Heightmap::new(7, config.heightmap.clone()),
            biome_config: config.biome.clone(),
            sea_level: config.sea_level,
            seed: 7,
            population,
            populated_chunks,
        }
    }

    #[test]
    fn packed_plant_is_sixteen_bytes() {
        assert_eq!(std::mem::size_of::<Plant>(), 16);
    }

    #[test]
    fn pack_round_trips_within_quantization_tolerance() {
        let origin_x = 5.0 * CHUNK_SIZE_METERS;
        let origin_z = 7.0 * CHUNK_SIZE_METERS;
        let original = PlantInstance {
            position: Vec3::new(origin_x + 123.4, 88.0, origin_z + 200.1),
            rotation: 2.0,
            height: 12.5,
            species_index: 3,
            growth_stage: GrowthStage::Mature,
        };
        let packed = Plant::pack(&original, origin_x, origin_z);

        // Reconstruct against flat terrain so y is well-defined.
        let total = crate::world_core::chunk::CHUNK_GRID_RESOLUTION
            * crate::world_core::chunk::CHUNK_GRID_RESOLUTION;
        let terrain = ChunkTerrain {
            heights: vec![88.0; total],
            moisture: vec![0.5; total],
            min_height: 88.0,
            max_height: 88.0,
            has_water: false,
        };
        let back = packed.to_instance(IVec2::new(5, 7), &terrain);

        assert!((back.position.x - original.position.x).abs() < 0.01);
        assert!((back.position.z - original.position.z).abs() < 0.01);
        assert!((back.position.y - 88.0).abs() < 1e-3);
        assert!((back.height - original.height).abs() < 0.01);
        assert!((back.rotation - original.rotation).abs() < 0.01);
        assert_eq!(back.species_index, original.species_index);
        assert_eq!(back.growth_stage, GrowthStage::Mature);
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
            min_height: 50.0,
            max_height: 50.0,
            has_water: false,
        };
        let plant = Plant {
            local_x: quantize_span(40.0),
            local_z: quantize_span(60.0),
            height: quantize_span(10.0),
            rotation: 0,
            species: 0,
            stage: MATURE,
            born_hour: 0.0,
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
    fn tick_growth_promotes_immature_plants_to_mature() {
        let reg = registry();
        let species = &reg.species[0];
        let mature_at = (species.placement.seedling_hours + species.placement.young_hours) as f64;

        let mut world = test_world(
            vec![vec![Plant {
                local_x: 0,
                local_z: 0,
                height: quantize_span(5.0),
                rotation: 0,
                species: 0,
                stage: stage_to_u8(GrowthStage::Seedling),
                born_hour: 0.0,
            }]],
            Arc::clone(&reg),
        );

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

        let ctx = SpreadContext {
            heightmap: &world.heightmap,
            registry: &reg,
            biome_config: &world.biome_config,
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
            order,
        };
        let mut plants = Vec::new();
        let mut crowd = vec![mk(0), mk(1), mk(2), mk(3), mk(4)];
        let accepted = land_chunk_candidates(&mut plants, &mut crowd, &ctx);
        assert_eq!(accepted, 1, "spacing must reject coincident candidates");
        assert_eq!(plants.len(), 1);

        // A second pass at the very same spot adds nothing — the gate is stable,
        // which is what makes the global spread terminate.
        let mut again = vec![mk(5)];
        assert_eq!(land_chunk_candidates(&mut plants, &mut again, &ctx), 0);
        assert_eq!(plants.len(), 1);
    }

    #[test]
    fn spread_emits_seedlings_for_mature_plants() {
        // A mature plant whose spread roll succeeds emits at least one candidate
        // anchored near it; immature plants emit none.
        let reg = registry();
        let n = WORLD_SIZE_CHUNKS;
        // Find a chunk+plant_index whose spread roll fires for species 0.
        let species0 = &reg.species[0];
        let chance = species0.placement.spread_chance.clamp(0.0, 1.0);
        let canon = (0..n)
            .flat_map(|z| (0..n).map(move |x| IVec2::new(x, z)))
            .find(|c| spread_roll(7, *c, 0) < chance)
            .expect("a chunk whose plant 0 spreads");
        let idx = (canon.y * n + canon.x) as usize;
        let mut chunks = vec![Vec::new(); (n * n) as usize];
        chunks[idx].push(Plant {
            local_x: quantize_span(128.0),
            local_z: quantize_span(128.0),
            height: quantize_span(10.0),
            rotation: 0,
            species: 0,
            stage: MATURE,
            born_hour: 0.0,
        });
        let mature = emit_chunk_candidates(idx, &chunks, &reg, 7);
        assert!(!mature.is_empty(), "a firing mature plant emits candidates");
        for c in &mature {
            // Candidate world position is within spread_radius of the source.
            let dx = c.world_x - (canon.x as f32 * CHUNK_SIZE_METERS + 128.0);
            let dz = c.world_z - (canon.y as f32 * CHUNK_SIZE_METERS + 128.0);
            assert!((dx * dx + dz * dz).sqrt() <= species0.placement.spread_radius + 0.01);
        }

        // The same plant as a seedling emits nothing.
        chunks[idx][0].stage = stage_to_u8(GrowthStage::Seedling);
        assert!(emit_chunk_candidates(idx, &chunks, &reg, 7).is_empty());
    }
}
