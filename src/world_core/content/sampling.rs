use crate::world_core::biome::Biome;
use crate::world_core::chunk::{CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS};

pub fn sample_biome_nearest(values: &[Biome], local_x: f32, local_z: f32) -> Biome {
    let side = CHUNK_GRID_RESOLUTION;
    let x = (((local_x / CHUNK_SIZE_METERS) * (side - 1) as f32).round() as i32)
        .clamp(0, (side - 1) as i32) as usize;
    let z = (((local_z / CHUNK_SIZE_METERS) * (side - 1) as f32).round() as i32)
        .clamp(0, (side - 1) as i32) as usize;
    values[z * side + x]
}

pub fn sample_field_bilinear(values: &[f32], local_x: f32, local_z: f32) -> f32 {
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

pub fn estimate_slope(heights: &[f32], local_x: f32, local_z: f32) -> f32 {
    let d = 1.75;
    let hx0 = sample_field_bilinear(heights, local_x - d, local_z);
    let hx1 = sample_field_bilinear(heights, local_x + d, local_z);
    let hz0 = sample_field_bilinear(heights, local_x, local_z - d);
    let hz1 = sample_field_bilinear(heights, local_x, local_z + d);
    let dx = (hx1 - hx0) / (2.0 * d);
    let dz = (hz1 - hz0) / (2.0 * d);
    (dx * dx + dz * dz).sqrt()
}

pub fn hash4(a: u32, b: u32, c: u32, d: u32) -> u32 {
    let mut x = a.wrapping_mul(0x9E37_79B9) ^ b.rotate_left(13) ^ c.rotate_left(7) ^ d;
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ (x >> 16)
}

pub fn hash_to_unit_float(v: u32) -> f32 {
    ((v as f64) / (u32::MAX as f64)) as f32
}
