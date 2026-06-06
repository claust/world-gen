use std::sync::Arc;

use glam::IVec2;
#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

use crate::world_core::chunk::{ChunkTerrain, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS};
use crate::world_core::config::HeightmapConfig;
use crate::world_core::heightmap::Heightmap;
use crate::world_core::layer::Layer;
use crate::world_core::rivers::RiverField;

pub struct TerrainLayer {
    heightmap: Heightmap,
    rivers: Arc<RiverField>,
    sea_level: f32,
}

impl TerrainLayer {
    pub fn new(
        seed: u32,
        heightmap_config: HeightmapConfig,
        sea_level: f32,
        rivers: Arc<RiverField>,
    ) -> Self {
        Self {
            heightmap: Heightmap::new(seed, heightmap_config),
            rivers,
            sea_level,
        }
    }
}

impl Layer<IVec2, ChunkTerrain> for TerrainLayer {
    fn generate(&self, coord: IVec2) -> ChunkTerrain {
        let side = CHUNK_GRID_RESOLUTION;
        let total = side * side;
        let cell_size = CHUNK_SIZE_METERS / (side - 1) as f32;
        let origin_x = coord.x as f32 * CHUNK_SIZE_METERS;
        let origin_z = coord.y as f32 * CHUNK_SIZE_METERS;

        // Sample raw height and the global river field together, carving the
        // channel into the height and keeping the wetness for the riverbed tint.
        let (heights, river): (Vec<f32>, Vec<f32>) = maybe_par_iter!(0..total)
            .map(|idx| {
                let x = idx % side;
                let z = idx / side;
                let world_x = origin_x + x as f32 * cell_size;
                let world_z = origin_z + z as f32 * cell_size;
                let raw = self.heightmap.sample_height(world_x, world_z);
                let (carve, wet) = self.rivers.sample(world_x, world_z);
                (raw - carve, wet)
            })
            .unzip();

        let moisture: Vec<f32> = maybe_par_iter!(0..total)
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
            heights,
            moisture,
            river,
            has_water: min_height < self.sea_level,
            min_height,
            max_height,
        }
    }
}
