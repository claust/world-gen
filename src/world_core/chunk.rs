use glam::IVec2;
use rayon::prelude::*;

use crate::world_core::heightmap::Heightmap;

pub const CHUNK_SIZE_METERS: f32 = 256.0;
pub const CHUNK_GRID_RESOLUTION: usize = 129;

#[derive(Clone)]
pub struct ChunkTerrain {
    pub coord: IVec2,
    pub heights: Vec<f32>,
    pub moisture: Vec<f32>,
    pub min_height: f32,
    pub max_height: f32,
}

pub struct ChunkGenerator {
    heightmap: Heightmap,
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            heightmap: Heightmap::new(seed),
        }
    }

    pub fn generate_chunk(&self, coord: IVec2) -> ChunkTerrain {
        let side = CHUNK_GRID_RESOLUTION;
        let total = side * side;
        let cell_size = CHUNK_SIZE_METERS / (side - 1) as f32;
        let origin_x = coord.x as f32 * CHUNK_SIZE_METERS;
        let origin_z = coord.y as f32 * CHUNK_SIZE_METERS;

        let heights: Vec<f32> = (0..total)
            .into_par_iter()
            .map(|idx| {
                let x = idx % side;
                let z = idx / side;
                let world_x = origin_x + x as f32 * cell_size;
                let world_z = origin_z + z as f32 * cell_size;
                self.heightmap.sample_height(world_x, world_z)
            })
            .collect();

        let moisture: Vec<f32> = (0..total)
            .into_par_iter()
            .map(|idx| {
                let x = idx % side;
                let z = idx / side;
                let world_x = origin_x + x as f32 * cell_size;
                let world_z = origin_z + z as f32 * cell_size;
                self.heightmap.sample_moisture(world_x, world_z)
            })
            .collect();

        let (min_height, max_height) = heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(min_h, max_h), h| {
                (min_h.min(*h), max_h.max(*h))
            });

        ChunkTerrain {
            coord,
            heights,
            moisture,
            min_height,
            max_height,
        }
    }
}
