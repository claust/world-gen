use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{IVec2, Vec3};
use rayon::prelude::*;

use crate::world_gen::biome;
use crate::world_gen::heightmap::Heightmap;

pub const CHUNK_SIZE_METERS: f32 = 256.0;
pub const CHUNK_GRID_RESOLUTION: usize = 129;

#[repr(C)]
#[derive(Clone, Copy, Debug, Zeroable, Pod)]
pub struct TerrainVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct ChunkData {
    pub coord: IVec2,
    pub vertices: Arc<Vec<TerrainVertex>>,
    pub indices: Arc<Vec<u32>>,
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

    pub fn generate_chunk(&self, coord: IVec2) -> ChunkData {
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

        let normals: Vec<Vec3> = (0..total)
            .into_par_iter()
            .map(|idx| {
                let x = idx % side;
                let z = idx / side;

                let x0 = x.saturating_sub(1);
                let x1 = (x + 1).min(side - 1);
                let z0 = z.saturating_sub(1);
                let z1 = (z + 1).min(side - 1);

                let h_l = heights[z * side + x0];
                let h_r = heights[z * side + x1];
                let h_d = heights[z0 * side + x];
                let h_u = heights[z1 * side + x];

                Vec3::new(h_l - h_r, cell_size * 2.0, h_d - h_u).normalize()
            })
            .collect();

        let vertices: Vec<TerrainVertex> = (0..total)
            .into_par_iter()
            .map(|idx| {
                let x = idx % side;
                let z = idx / side;
                let world_x = origin_x + x as f32 * cell_size;
                let world_z = origin_z + z as f32 * cell_size;
                let h = heights[idx];
                let biome = biome::classify(h, moisture[idx]);
                let color = biome::ground_color(biome, h);
                let n = normals[idx];
                TerrainVertex {
                    position: [world_x, h, world_z],
                    normal: [n.x, n.y, n.z],
                    color: [color.x, color.y, color.z],
                }
            })
            .collect();

        let mut indices = Vec::with_capacity((side - 1) * (side - 1) * 6);
        for z in 0..(side - 1) {
            for x in 0..(side - 1) {
                let i0 = (z * side + x) as u32;
                let i1 = i0 + 1;
                let i2 = i0 + side as u32;
                let i3 = i2 + 1;
                indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
            }
        }

        let (min_height, max_height) = heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(min_h, max_h), h| {
                (min_h.min(*h), max_h.max(*h))
            });

        ChunkData {
            coord,
            vertices: Arc::new(vertices),
            indices: Arc::new(indices),
            min_height,
            max_height,
        }
    }
}
