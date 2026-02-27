use glam::{IVec2, Vec3};

use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    ChunkTerrain, TreeInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};
use crate::world_core::layer::Layer;

pub struct FloraLayer {
    seed: u32,
}

pub struct FloraInput<'a> {
    pub coord: IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

impl FloraLayer {
    pub fn new(seed: u32) -> Self {
        Self { seed }
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
        let spacing = 11.0;
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
                let density = biome_tree_density(biome, moisture);
                if rnd > density {
                    continue;
                }

                let slope = estimate_slope(&terrain.heights, local_x, local_z);
                if slope > 1.0 || height < -20.0 {
                    continue;
                }

                let trunk_height = 4.5
                    + hash_to_unit_float(hash4(
                        self.seed.wrapping_add(401),
                        coord.x as u32,
                        coord.y as u32,
                        cell_id,
                    )) * 7.5;
                let canopy_radius = 1.7
                    + hash_to_unit_float(hash4(
                        self.seed.wrapping_add(809),
                        coord.x as u32,
                        coord.y as u32,
                        cell_id,
                    )) * 2.5;

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

fn biome_tree_density(biome: Biome, moisture: f32) -> f32 {
    match biome {
        Biome::Forest => (0.42 + (moisture - 0.62) * 0.7).clamp(0.30, 0.72),
        Biome::Grassland => (0.02 + moisture * 0.08).clamp(0.01, 0.11),
        Biome::Rock | Biome::Desert | Biome::Snow => 0.0,
    }
}

fn sample_biome_nearest(values: &[Biome], local_x: f32, local_z: f32) -> Biome {
    let side = CHUNK_GRID_RESOLUTION;
    let x = (((local_x / CHUNK_SIZE_METERS) * (side - 1) as f32).round() as i32)
        .clamp(0, (side - 1) as i32) as usize;
    let z = (((local_z / CHUNK_SIZE_METERS) * (side - 1) as f32).round() as i32)
        .clamp(0, (side - 1) as i32) as usize;
    values[z * side + x]
}

fn sample_field_bilinear(values: &[f32], local_x: f32, local_z: f32) -> f32 {
    let side = CHUNK_GRID_RESOLUTION;
    let xf = ((local_x / CHUNK_SIZE_METERS) * (side - 1) as f32).clamp(0.0, (side - 1) as f32);
    let zf = ((local_z / CHUNK_SIZE_METERS) * (side - 1) as f32).clamp(0.0, (side - 1) as f32);

    let x0 = xf.floor() as usize;
    let z0 = zf.floor() as usize;
    let x1 = (x0 + 1).min(side - 1);
    let z1 = (z0 + 1).min(side - 1);
    let tx = xf - x0 as f32;
    let tz = zf - z0 as f32;

    let h00 = values[z0 * side + x0];
    let h10 = values[z0 * side + x1];
    let h01 = values[z1 * side + x0];
    let h11 = values[z1 * side + x1];

    let hx0 = h00 + (h10 - h00) * tx;
    let hx1 = h01 + (h11 - h01) * tx;
    hx0 + (hx1 - hx0) * tz
}

fn estimate_slope(heights: &[f32], local_x: f32, local_z: f32) -> f32 {
    let d = 1.75;
    let hx0 = sample_field_bilinear(heights, local_x - d, local_z);
    let hx1 = sample_field_bilinear(heights, local_x + d, local_z);
    let hz0 = sample_field_bilinear(heights, local_x, local_z - d);
    let hz1 = sample_field_bilinear(heights, local_x, local_z + d);
    let dx = (hx1 - hx0) / (2.0 * d);
    let dz = (hz1 - hz0) / (2.0 * d);
    (dx * dx + dz * dz).sqrt()
}

fn hash4(a: u32, b: u32, c: u32, d: u32) -> u32 {
    let mut x = a.wrapping_mul(0x9E37_79B9) ^ b.rotate_left(13) ^ c.rotate_left(7) ^ d;
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ (x >> 16)
}

fn hash_to_unit_float(v: u32) -> f32 {
    ((v as f64) / (u32::MAX as f64)) as f32
}
