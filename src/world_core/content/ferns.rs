use glam::{IVec2, Vec3};

use super::sampling::{
    estimate_slope, hash4, hash_to_unit_float, sample_biome_nearest, sample_field_bilinear,
};
use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    ChunkTerrain, FernInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};
use crate::world_core::config::FernsConfig;
use crate::world_core::layer::Layer;

pub struct FernsLayer {
    seed: u32,
    config: FernsConfig,
    sea_level: f32,
}

pub struct FernsInput<'a> {
    pub coord: IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

impl FernsLayer {
    pub fn new(seed: u32, config: FernsConfig, sea_level: f32) -> Self {
        Self {
            seed,
            config,
            sea_level,
        }
    }
}

impl<'a> Layer<FernsInput<'a>, Vec<FernInstance>> for FernsLayer {
    fn generate(&self, input: FernsInput<'a>) -> Vec<FernInstance> {
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

        let mut ferns = Vec::new();
        let spacing = self.config.grid_spacing;
        let cells_per_side = (CHUNK_SIZE_METERS / spacing) as i32;

        for gz in 0..cells_per_side {
            for gx in 0..cells_per_side {
                let cell_id = ((gx as u32) << 16) | (gz as u32);
                let rnd = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(2001),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                ));

                let jitter_x = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(2071),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                ));
                let jitter_z = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(2193),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                ));

                let local_x = (gx as f32 + jitter_x) * spacing;
                let local_z = (gz as f32 + jitter_z) * spacing;

                let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
                let moisture = sample_field_bilinear(&terrain.moisture, local_x, local_z);
                let biome = sample_biome_nearest(&biome_map.values, local_x, local_z);
                let density = self.fern_density(biome, moisture);
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

                let rotation = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(2401),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                )) * std::f32::consts::TAU;

                let scale = self.config.scale_min
                    + hash_to_unit_float(hash4(
                        self.seed.wrapping_add(2501),
                        coord.x as u32,
                        coord.y as u32,
                        cell_id,
                    )) * self.config.scale_range;

                let world_x = coord.x as f32 * CHUNK_SIZE_METERS + local_x;
                let world_z = coord.y as f32 * CHUNK_SIZE_METERS + local_z;
                ferns.push(FernInstance {
                    position: Vec3::new(world_x, height, world_z),
                    rotation,
                    scale,
                });
            }
        }

        ferns
    }
}

impl FernsLayer {
    fn fern_density(&self, biome: Biome, moisture: f32) -> f32 {
        let c = &self.config;
        match biome {
            Biome::Forest => ((moisture - c.forest_density_offset) * c.forest_density_scale)
                .clamp(0.0, c.forest_density_max),
            Biome::Grassland => ((moisture - c.grassland_density_offset)
                * c.grassland_density_scale)
                .clamp(0.0, c.grassland_density_max),
            Biome::Rock | Biome::Desert | Biome::Snow => 0.0,
        }
    }
}
