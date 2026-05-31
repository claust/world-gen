//! Resident, whole-world plant store.
//!
//! Iteration 1 of the live simulation keeps every plant in every one of the
//! `WORLD_SIZE_CHUNKS²` canonical chunks resident and simulates them on one
//! global clock — see [`docs/LIVE_SIM_ITERATION_1.md`]. This module is the
//! store: a packed `Plant` per canonical chunk, generated once at world
//! creation, read by the render bridge, and ticked for growth on the global
//! clock. Spread (M3) appends into it; persistence (M4/M5) saves it.

use std::sync::Arc;

use glam::{IVec2, Vec3};

use crate::world_core::chunk::{
    canonical_chunk, ChunkTerrain, PlantInstance, CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS,
};
use crate::world_core::chunk_generator::ChunkGenerator;
use crate::world_core::config::GameConfig;
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

impl Plant {
    /// Pack a generated plant whose absolute position lies in the chunk whose
    /// world origin is `(origin_x, origin_z)`.
    fn pack(plant: &PlantInstance, origin_x: f32, origin_z: f32) -> Self {
        let rotation = (plant.rotation.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
            * u16::MAX as f32)
            .round()
            .clamp(0.0, u16::MAX as f32) as u16;
        Plant {
            local_x: quantize_span(plant.position.x - origin_x),
            local_z: quantize_span(plant.position.z - origin_z),
            height: quantize_span(plant.height),
            rotation,
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
    /// fully-grown chunks (all of them, before spread exists).
    immature: Vec<u32>,
    registry: Arc<PlantRegistry>,
    /// Cached totals so `stats()` (called every frame) stays O(1). Maintained as
    /// chunks change; spread (M3) updates them as chunks gain plants / transition
    /// empty↔non-empty.
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
            use rayon::prelude::*;
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

        Self {
            chunks,
            immature,
            registry,
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
    /// so this only walks chunks that still hold immature plants. With base-only
    /// flora (all mature) it does nothing; M3 spread makes it live.
    pub fn tick_growth(&mut self, total_hours: f64) {
        for (idx, chunk) in self.chunks.iter_mut().enumerate() {
            if self.immature[idx] == 0 {
                continue;
            }
            let mut immature = 0u32;
            for plant in chunk.iter_mut() {
                if plant.stage != MATURE {
                    plant.stage = stage_to_u8(stage_for(plant, total_hours, &self.registry));
                    if plant.stage != MATURE {
                        immature += 1;
                    }
                }
            }
            self.immature[idx] = immature;
        }
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
    fn tick_growth_promotes_immature_plants_to_mature() {
        let reg = registry();
        let species = &reg.species[0];
        let mature_at = (species.placement.seedling_hours + species.placement.young_hours) as f64;

        let mut world = PlantWorld {
            chunks: vec![vec![Plant {
                local_x: 0,
                local_z: 0,
                height: quantize_span(5.0),
                rotation: 0,
                species: 0,
                stage: stage_to_u8(GrowthStage::Seedling),
                born_hour: 0.0,
            }]],
            immature: vec![1],
            registry: Arc::clone(&reg),
            population: 1,
            populated_chunks: 1,
        };

        // Before any growth time, it stays a seedling.
        world.tick_growth(0.0);
        assert_eq!(world.chunks[0][0].stage, stage_to_u8(GrowthStage::Seedling));
        assert_eq!(world.immature[0], 1);

        // Well past maturity, it becomes mature and the chunk drops to zero
        // immature — so later ticks skip it.
        world.tick_growth(mature_at + 1.0);
        assert_eq!(world.chunks[0][0].stage, MATURE);
        assert_eq!(world.immature[0], 0);
    }
}
