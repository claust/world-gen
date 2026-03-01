use glam::{IVec2, Vec3};

use super::sampling::{
    estimate_slope, hash4, hash_to_unit_float, sample_biome_nearest, sample_field_bilinear,
};
use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    ChunkTerrain, TreeInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};
use crate::world_core::config::FloraConfig;
use crate::world_core::layer::Layer;

pub struct FloraLayer {
    seed: u32,
    config: FloraConfig,
    sea_level: f32,
}

pub struct FloraInput<'a> {
    pub coord: IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

impl FloraLayer {
    pub fn new(seed: u32, config: FloraConfig, sea_level: f32) -> Self {
        Self {
            seed,
            config,
            sea_level,
        }
    }
}

impl<'a> Layer<FloraInput<'a>, Vec<TreeInstance>> for FloraLayer {
    fn generate(&self, input: FloraInput<'a>) -> Vec<TreeInstance> {
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

        let mut trees = Vec::new();
        let spacing = self.config.grid_spacing;
        let cells_per_side = (CHUNK_SIZE_METERS / spacing) as i32;

        for gz in 0..cells_per_side {
            for gx in 0..cells_per_side {
                let cell_id = ((gx as u32) << 16) | (gz as u32);
                let rnd =
                    hash_to_unit_float(hash4(self.seed, coord.x as u32, coord.y as u32, cell_id));

                let jitter_x = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(71),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                ));
                let jitter_z = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(193),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                ));

                let local_x = (gx as f32 + jitter_x) * spacing;
                let local_z = (gz as f32 + jitter_z) * spacing;

                let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
                let moisture = sample_field_bilinear(&terrain.moisture, local_x, local_z);
                let biome = sample_biome_nearest(&biome_map.values, local_x, local_z);
                let density = self.biome_tree_density(biome, moisture);
                if rnd > density {
                    continue;
                }

                let slope = estimate_slope(&terrain.heights, local_x, local_z);
                if slope > self.config.max_slope
                    || height < self.config.min_height
                    || height < self.sea_level
                {
                    continue;
                }

                let trunk_height = self.config.trunk_height_min
                    + hash_to_unit_float(hash4(
                        self.seed.wrapping_add(401),
                        coord.x as u32,
                        coord.y as u32,
                        cell_id,
                    )) * self.config.trunk_height_range;
                let canopy_radius = self.config.canopy_radius_min
                    + hash_to_unit_float(hash4(
                        self.seed.wrapping_add(809),
                        coord.x as u32,
                        coord.y as u32,
                        cell_id,
                    )) * self.config.canopy_radius_range;

                let world_x = coord.x as f32 * CHUNK_SIZE_METERS + local_x;
                let world_z = coord.y as f32 * CHUNK_SIZE_METERS + local_z;
                trees.push(TreeInstance {
                    position: Vec3::new(world_x, height, world_z),
                    trunk_height,
                    canopy_radius,
                });
            }
        }

        trees
    }
}

impl FloraLayer {
    fn biome_tree_density(&self, biome: Biome, moisture: f32) -> f32 {
        let c = &self.config;
        match biome {
            Biome::Forest => (c.forest_density_base
                + (moisture - c.forest_moisture_center) * c.forest_density_scale)
                .clamp(c.forest_density_min, c.forest_density_max),
            Biome::Grassland => (c.grassland_density_base + moisture * c.grassland_density_scale)
                .clamp(c.grassland_density_min, c.grassland_density_max),
            Biome::Rock | Biome::Desert | Biome::Snow => 0.0,
        }
    }
}
