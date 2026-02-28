use glam::{IVec2, Vec3};

use super::sampling::{
    estimate_slope, hash4, hash_to_unit_float, sample_biome_nearest, sample_field_bilinear,
};
use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    ChunkTerrain, HouseInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};
use crate::world_core::layer::Layer;

pub struct HousesLayer {
    seed: u32,
}

pub struct HousesInput<'a> {
    pub coord: IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

impl HousesLayer {
    pub fn new(seed: u32) -> Self {
        Self { seed }
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

        let mut houses = Vec::new();
        let spacing = 40.0;
        let cells_per_side = (CHUNK_SIZE_METERS / spacing) as i32;

        for gz in 0..cells_per_side {
            for gx in 0..cells_per_side {
                let cell_id = ((gx as u32) << 16) | (gz as u32);
                let rnd = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(1001),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                ));

                let jitter_x = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(1071),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                ));
                let jitter_z = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(1193),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                ));

                let local_x = (gx as f32 + jitter_x) * spacing;
                let local_z = (gz as f32 + jitter_z) * spacing;

                let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
                let biome = sample_biome_nearest(&biome_map.values, local_x, local_z);

                let density = match biome {
                    Biome::Grassland => 0.04,
                    _ => 0.0,
                };
                if rnd > density {
                    continue;
                }

                let slope = estimate_slope(&terrain.heights, local_x, local_z);
                if slope > 0.3 || !(0.0..=100.0).contains(&height) {
                    continue;
                }

                let rotation = hash_to_unit_float(hash4(
                    self.seed.wrapping_add(1401),
                    coord.x as u32,
                    coord.y as u32,
                    cell_id,
                )) * std::f32::consts::TAU;

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
