use glam::{IVec2, Vec3};

use super::sampling::{
    estimate_slope, hash4, hash_to_unit_float, sample_biome_nearest, sample_field_bilinear,
};
use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    ChunkTerrain, HouseInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};
use crate::world_core::config::HousesConfig;
use crate::world_core::layer::Layer;

pub struct HousesLayer {
    seed: u32,
    config: HousesConfig,
    sea_level: f32,
}

pub struct HousesInput<'a> {
    pub coord: IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

impl HousesLayer {
    pub fn new(seed: u32, config: HousesConfig, sea_level: f32) -> Self {
        Self {
            seed,
            config,
            sea_level,
        }
    }

    fn is_valid_site(
        &self,
        terrain: &ChunkTerrain,
        biome_map: &BiomeMap,
        local_x: f32,
        local_z: f32,
    ) -> bool {
        if local_x < 0.0
            || local_z < 0.0
            || local_x >= CHUNK_SIZE_METERS
            || local_z >= CHUNK_SIZE_METERS
        {
            return false;
        }
        let biome = sample_biome_nearest(&biome_map.values, local_x, local_z);
        if biome != Biome::Grassland {
            return false;
        }
        let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
        let slope = estimate_slope(&terrain.heights, local_x, local_z);
        slope <= self.config.max_slope
            && height >= self.sea_level
            && (self.config.height_min..=self.config.height_max).contains(&height)
    }

    fn height_at(&self, terrain: &ChunkTerrain, local_x: f32, local_z: f32) -> f32 {
        sample_field_bilinear(&terrain.heights, local_x, local_z)
    }
}

impl<'a> Layer<HousesInput<'a>, Vec<HouseInstance>> for HousesLayer {
    fn generate(&self, input: HousesInput<'a>) -> Vec<HouseInstance> {
        let coord = input.coord;
        let terrain = input.terrain;
        let biome_map = input.biome_map;

        let total = CHUNK_GRID_RESOLUTION * CHUNK_GRID_RESOLUTION;
        if terrain.heights.len() != total
            || terrain.moisture.len() != total
            || biome_map.values.len() != total
        {
            return Vec::new();
        }

        let cx = coord.x as u32;
        let cy = coord.y as u32;
        let mut houses = Vec::new();

        // Phase 1: Hamlet clusters on a coarse grid
        let hamlet_spacing = self.config.hamlet_spacing;
        let hamlet_cells = (CHUNK_SIZE_METERS / hamlet_spacing) as i32;

        for gz in 0..hamlet_cells {
            for gx in 0..hamlet_cells {
                let cell_id = ((gx as u32) << 16) | (gz as u32);

                let rnd = hash_to_unit_float(hash4(self.seed.wrapping_add(3001), cx, cy, cell_id));
                if rnd > self.config.hamlet_density {
                    continue;
                }

                // Hamlet center position (with jitter)
                let jx = hash_to_unit_float(hash4(self.seed.wrapping_add(3071), cx, cy, cell_id));
                let jz = hash_to_unit_float(hash4(self.seed.wrapping_add(3093), cx, cy, cell_id));
                let center_x = (gx as f32 + jx) * hamlet_spacing;
                let center_z = (gz as f32 + jz) * hamlet_spacing;

                if !self.is_valid_site(terrain, biome_map, center_x, center_z) {
                    continue;
                }

                // Determine house count for this hamlet
                let range = self.config.hamlet_house_max - self.config.hamlet_house_min + 1;
                let count_rnd = hash4(self.seed.wrapping_add(3201), cx, cy, cell_id);
                let count = self.config.hamlet_house_min + (count_rnd % range);

                for i in 0..count {
                    let sub_id = cell_id.wrapping_mul(31).wrapping_add(i);

                    let angle =
                        hash_to_unit_float(hash4(self.seed.wrapping_add(3301), cx, cy, sub_id))
                            * std::f32::consts::TAU;

                    // sqrt for uniform disk distribution
                    let raw_dist =
                        hash_to_unit_float(hash4(self.seed.wrapping_add(3401), cx, cy, sub_id));
                    let dist = raw_dist.sqrt() * self.config.hamlet_radius;

                    let local_x = center_x + angle.cos() * dist;
                    let local_z = center_z + angle.sin() * dist;

                    if !self.is_valid_site(terrain, biome_map, local_x, local_z) {
                        continue;
                    }

                    let height = self.height_at(terrain, local_x, local_z);
                    let rotation =
                        hash_to_unit_float(hash4(self.seed.wrapping_add(3501), cx, cy, sub_id))
                            * std::f32::consts::TAU;

                    let world_x = coord.x as f32 * CHUNK_SIZE_METERS + local_x;
                    let world_z = coord.y as f32 * CHUNK_SIZE_METERS + local_z;
                    houses.push(HouseInstance {
                        position: Vec3::new(world_x, height, world_z),
                        rotation,
                    });
                }
            }
        }

        // Phase 2: Solo houses on the fine grid (lower density)
        let spacing = self.config.grid_spacing;
        let cells_per_side = (CHUNK_SIZE_METERS / spacing) as i32;

        for gz in 0..cells_per_side {
            for gx in 0..cells_per_side {
                let cell_id = ((gx as u32) << 16) | (gz as u32);
                let rnd = hash_to_unit_float(hash4(self.seed.wrapping_add(1001), cx, cy, cell_id));

                let jitter_x =
                    hash_to_unit_float(hash4(self.seed.wrapping_add(1071), cx, cy, cell_id));
                let jitter_z =
                    hash_to_unit_float(hash4(self.seed.wrapping_add(1193), cx, cy, cell_id));

                let local_x = (gx as f32 + jitter_x) * spacing;
                let local_z = (gz as f32 + jitter_z) * spacing;

                let biome = sample_biome_nearest(&biome_map.values, local_x, local_z);
                let density = match biome {
                    Biome::Grassland => self.config.grassland_density,
                    _ => 0.0,
                };
                if rnd > density {
                    continue;
                }

                let slope = estimate_slope(&terrain.heights, local_x, local_z);
                let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
                if slope > self.config.max_slope
                    || height < self.sea_level
                    || !(self.config.height_min..=self.config.height_max).contains(&height)
                {
                    continue;
                }

                let rotation =
                    hash_to_unit_float(hash4(self.seed.wrapping_add(1401), cx, cy, cell_id))
                        * std::f32::consts::TAU;

                let world_x = coord.x as f32 * CHUNK_SIZE_METERS + local_x;
                let world_z = coord.y as f32 * CHUNK_SIZE_METERS + local_z;
                houses.push(HouseInstance {
                    position: Vec3::new(world_x, height, world_z),
                    rotation,
                });
            }
        }

        houses
    }
}
