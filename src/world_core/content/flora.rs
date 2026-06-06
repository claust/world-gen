use glam::{IVec2, Vec3};

use super::sampling::{
    estimate_slope, hash4, hash_to_unit_float, sample_biome_nearest, sample_field_bilinear,
};
use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    canonical_chunk, ChunkTerrain, PlantInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::layer::Layer;
use crate::world_core::lifecycle::GrowthStage;
use crate::world_core::rivers::{AQUATIC_MAX_WETNESS, MAX_PLANTABLE_WETNESS};

use std::sync::Arc;

pub struct FloraLayer {
    seed: u32,
    sea_level: f32,
    registry: Arc<PlantRegistry>,
}

pub struct FloraInput<'a> {
    pub coord: IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

impl FloraLayer {
    pub fn new(seed: u32, sea_level: f32, registry: Arc<PlantRegistry>) -> Self {
        Self {
            seed,
            sea_level,
            registry,
        }
    }
}

impl<'a> Layer<FloraInput<'a>, Vec<PlantInstance>> for FloraLayer {
    fn generate(&self, input: FloraInput<'a>) -> Vec<PlantInstance> {
        let coord = input.coord;
        let terrain = input.terrain;
        let biome_map = input.biome_map;

        let total = CHUNK_GRID_RESOLUTION * CHUNK_GRID_RESOLUTION;
        if terrain.heights.len() != total
            || terrain.moisture.len() != total
            || terrain.river.len() != total
            || biome_map.values.len() != total
        {
            return Vec::new();
        }

        let mut plants = Vec::new();

        // Two land grid passes: trees (11m spacing) and shrubs (4m spacing)
        self.place_grid(coord, terrain, biome_map, 11.0, "tree", &mut plants);
        self.place_grid(coord, terrain, biome_map, 4.0, "shrub", &mut plants);
        // Aquatic pass: cattails fringe the wet bank band the land passes avoid.
        self.place_aquatic_grid(coord, terrain, biome_map, 5.0, &mut plants);

        plants
    }
}

impl FloraLayer {
    fn place_grid(
        &self,
        coord: IVec2,
        terrain: &ChunkTerrain,
        biome_map: &BiomeMap,
        spacing: f32,
        kind_filter: &str,
        plants: &mut Vec<PlantInstance>,
    ) {
        // Use a different seed offset for shrubs to avoid grid overlap
        let seed_offset: u32 = if kind_filter == "shrub" { 2000 } else { 0 };

        // Hash on the canonical (wrapped) chunk id so chunks a whole number of
        // world laps apart grow identical vegetation; world positions below stay
        // on the raw `coord` so placement follows the camera's actual location.
        let canon = canonical_chunk(coord);

        let cells_per_side = (CHUNK_SIZE_METERS / spacing) as i32;
        let mut eligible: Vec<(usize, f32)> = Vec::with_capacity(self.registry.species.len());

        for gz in 0..cells_per_side {
            for gx in 0..cells_per_side {
                let cell_id = ((gx as u32) << 16) | (gz as u32);
                let rnd = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ));

                let jitter_x = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(71 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ));
                let jitter_z = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(193 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ));

                let local_x = (gx as f32 + jitter_x) * spacing;
                let local_z = (gz as f32 + jitter_z) * spacing;

                let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
                let moisture = sample_field_bilinear(&terrain.moisture, local_x, local_z);
                let biome = sample_biome_nearest(&biome_map.values, local_x, local_z);
                let slope = estimate_slope(&terrain.heights, local_x, local_z);

                if height < self.sea_level {
                    continue;
                }

                // Keep plants out of river channels. The sea-level test above
                // misses them: rivers carve below the surrounding terrain but
                // usually stay above sea level, and their raised moisture would
                // otherwise *boost* density right where the water flows.
                let wetness = sample_field_bilinear(&terrain.river, local_x, local_z);
                if wetness > MAX_PLANTABLE_WETNESS {
                    continue;
                }

                // Compute base density from biome (reuse old forest/grassland model)
                let base_density = biome_density(biome, moisture);
                if rnd > base_density {
                    continue;
                }

                // Determine near-water status
                let above_water = height - self.sea_level;
                let near_water = (0.0..8.0).contains(&above_water);

                // Filter eligible species and compute weighted selection
                let biome_str = biome_to_str(biome);
                eligible.clear();
                for (i, s) in self.registry.species.iter().enumerate() {
                    if s.kind != kind_filter {
                        continue;
                    }
                    // Aquatic species are placed by the dedicated aquatic pass.
                    if s.placement.aquatic {
                        continue;
                    }
                    if !s.placement.biomes.iter().any(|b| b == biome_str) {
                        continue;
                    }
                    if moisture < s.placement.min_moisture
                        || moisture > s.placement.max_moisture
                        || height < s.placement.min_altitude
                        || height > s.placement.max_altitude
                        || slope > s.placement.max_slope
                    {
                        continue;
                    }
                    let w = (s.placement.weight
                        + if near_water {
                            s.placement.near_water_boost
                        } else {
                            0.0
                        })
                    .max(0.0);
                    eligible.push((i, w));
                }

                if eligible.is_empty() {
                    continue;
                }

                // Weighted random selection
                let total_weight: f32 = eligible.iter().map(|(_, w)| w).sum();
                if total_weight <= 0.0 {
                    continue;
                }
                let select_rnd = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(500 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                )) * total_weight;

                let mut accum = 0.0;
                let mut selected_idx = eligible[0].0;
                for &(idx, w) in &eligible {
                    accum += w;
                    if select_rnd <= accum {
                        selected_idx = idx;
                        break;
                    }
                }

                let species = &self.registry.species[selected_idx];

                // Generate height from species range
                let height_rnd = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(401 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ));
                let plant_height = species.height_range[0]
                    + height_rnd * (species.height_range[1] - species.height_range[0]);

                let rotation = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(1117 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                )) * std::f32::consts::TAU;

                let world_x = coord.x as f32 * CHUNK_SIZE_METERS + local_x;
                let world_z = coord.y as f32 * CHUNK_SIZE_METERS + local_z;
                plants.push(PlantInstance {
                    position: Vec3::new(world_x, height, world_z),
                    rotation,
                    height: plant_height,
                    species_index: selected_idx,
                    growth_stage: GrowthStage::Mature,
                });
            }
        }
    }

    /// Place aquatic species (cattails) in the wet bank band that the land pass
    /// avoids. The mirror image of [`Self::place_grid`]'s guards: cells must be
    /// *wet* (`MAX_PLANTABLE_WETNESS..=AQUATIC_MAX_WETNESS`) rather than dry, and
    /// the below-sea-level skip is lifted so reeds can stand in shallow water.
    /// The wet band is a contiguous strip along each bank, so scattering on a
    /// fine grid inside it naturally produces reed beds.
    fn place_aquatic_grid(
        &self,
        coord: IVec2,
        terrain: &ChunkTerrain,
        biome_map: &BiomeMap,
        spacing: f32,
        plants: &mut Vec<PlantInstance>,
    ) {
        // Per-cell spawn probability inside the wet band. Tuned for natural
        // scatter (not a perfect lattice) while keeping the clump count modest.
        const AQUATIC_DENSITY: f32 = 0.45;
        // Distinct seed offset so the aquatic grid never aliases tree(0)/shrub(2000).
        let seed_offset: u32 = 5000;

        let canon = canonical_chunk(coord);
        let cells_per_side = (CHUNK_SIZE_METERS / spacing) as i32;
        let mut eligible: Vec<(usize, f32)> = Vec::with_capacity(self.registry.species.len());

        for gz in 0..cells_per_side {
            for gx in 0..cells_per_side {
                let cell_id = ((gx as u32) << 16) | (gz as u32);
                let rnd = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ));
                let jitter_x = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(71 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ));
                let jitter_z = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(193 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ));

                let local_x = (gx as f32 + jitter_x) * spacing;
                let local_z = (gz as f32 + jitter_z) * spacing;

                // The wet band, mirrored from the land guard: skip dry cells and
                // the deep channel centre. No sea-level skip — reeds stand in
                // shallow water.
                let wetness = sample_field_bilinear(&terrain.river, local_x, local_z);
                if !(MAX_PLANTABLE_WETNESS..=AQUATIC_MAX_WETNESS).contains(&wetness) {
                    continue;
                }

                if rnd > AQUATIC_DENSITY {
                    continue;
                }

                let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
                let moisture = sample_field_bilinear(&terrain.moisture, local_x, local_z);
                let biome = sample_biome_nearest(&biome_map.values, local_x, local_z);
                let slope = estimate_slope(&terrain.heights, local_x, local_z);
                let biome_str = biome_to_str(biome);

                eligible.clear();
                for (i, s) in self.registry.species.iter().enumerate() {
                    if !s.placement.aquatic {
                        continue;
                    }
                    if !s.placement.biomes.iter().any(|b| b == biome_str) {
                        continue;
                    }
                    if moisture < s.placement.min_moisture
                        || moisture > s.placement.max_moisture
                        || height < s.placement.min_altitude
                        || height > s.placement.max_altitude
                        || slope > s.placement.max_slope
                    {
                        continue;
                    }
                    eligible.push((i, s.placement.weight.max(0.0)));
                }

                if eligible.is_empty() {
                    continue;
                }

                let total_weight: f32 = eligible.iter().map(|(_, w)| w).sum();
                if total_weight <= 0.0 {
                    continue;
                }
                let select_rnd = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(500 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                )) * total_weight;

                let mut accum = 0.0;
                let mut selected_idx = eligible[0].0;
                for &(idx, w) in &eligible {
                    accum += w;
                    if select_rnd <= accum {
                        selected_idx = idx;
                        break;
                    }
                }

                let species = &self.registry.species[selected_idx];

                let height_rnd = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(401 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ));
                let plant_height = species.height_range[0]
                    + height_rnd * (species.height_range[1] - species.height_range[0]);

                let rotation = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(1117 + seed_offset),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                )) * std::f32::consts::TAU;

                let world_x = coord.x as f32 * CHUNK_SIZE_METERS + local_x;
                let world_z = coord.y as f32 * CHUNK_SIZE_METERS + local_z;
                plants.push(PlantInstance {
                    position: Vec3::new(world_x, height, world_z),
                    rotation,
                    height: plant_height,
                    species_index: selected_idx,
                    growth_stage: GrowthStage::Mature,
                });
            }
        }
    }
}

fn biome_density(biome: Biome, moisture: f32) -> f32 {
    match biome {
        Biome::Forest => (0.42 + (moisture - 0.62) * 0.7).clamp(0.30, 0.72),
        Biome::Grassland => (0.02 + moisture * 0.08).clamp(0.01, 0.11),
        Biome::Desert => (0.01 + moisture * 0.03).clamp(0.0, 0.04),
        Biome::Rock | Biome::Snow => 0.0,
    }
}

fn biome_to_str(biome: Biome) -> &'static str {
    match biome {
        Biome::Forest => "Forest",
        Biome::Grassland => "Grassland",
        Biome::Desert => "Desert",
        Biome::Rock => "Rock",
        Biome::Snow => "Snow",
    }
}
