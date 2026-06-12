use std::sync::Arc;

use glam::IVec2;

use crate::world_core::chunk::{
    ChunkTerrain, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS, RIVER_SURFACE_THRESHOLD,
};
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

        // The terrain field is dominated by 4D simplex-noise sampling. Generate
        // the whole grid through the trig-hoisting grid samplers (bit-identical
        // to per-vertex `sample_height`/`sample_moisture`, but the per-axis torus
        // trig is computed once per row/column instead of once per vertex).
        let raw_heights = self
            .heightmap
            .height_grid(origin_x, origin_z, side, cell_size);
        let moisture = self
            .heightmap
            .moisture_grid(origin_x, origin_z, side, cell_size);

        // Carve the global river channel into the height and keep the wetness for
        // the riverbed tint, folding the min/max scan into the same pass. River
        // sampling is a cheap bilinear lookup, so a serial walk here is fine.
        let mut heights = Vec::with_capacity(total);
        let mut river = Vec::with_capacity(total);
        let mut river_flow = Vec::with_capacity(total);
        let mut river_surface = Vec::with_capacity(total);
        let mut min_height = f32::MAX;
        let mut max_height = f32::MIN;
        let mut has_river = false;
        for (idx, &raw) in raw_heights.iter().enumerate() {
            let x = idx % side;
            let z = idx / side;
            let world_x = origin_x + x as f32 * cell_size;
            let world_z = origin_z + z as f32 * cell_size;
            let (h, wet, sheet) = self.rivers.sample(raw, world_x, world_z);
            heights.push(h);
            river.push(wet);
            // Flat water sheet, already contained to the carve corridor by the
            // river field. Stored for every vertex; the renderer only draws it
            // where wetness marks a channel and the sheet rises above the carved
            // bed (`river_surface > heights`).
            river_surface.push(sheet);
            // Flow direction only matters where there is actually a river surface
            // to animate; skip the second lookup for dry vertices.
            if wet > RIVER_SURFACE_THRESHOLD {
                has_river = true;
                let (fx, fz) = self.rivers.sample_flow(world_x, world_z);
                river_flow.push([fx, fz]);
            } else {
                river_flow.push([0.0, 0.0]);
            }
            min_height = min_height.min(h);
            max_height = max_height.max(h);
        }

        ChunkTerrain {
            heights,
            moisture,
            river,
            river_flow,
            river_surface,
            has_water: min_height < self.sea_level,
            has_river,
            min_height,
            max_height,
        }
    }
}
