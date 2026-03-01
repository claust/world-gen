use crate::world_core::chunk::SEA_LEVEL;

/// CPU port of `biome_color()` from `shaders/terrain_gen.wgsl`.
/// Returns an RGBA color for the given height and moisture values.
pub fn biome_color_rgba(height: f32, moisture: f32) -> [u8; 4] {
    let base: [f32; 3] = if height < SEA_LEVEL {
        [0.15, 0.30, 0.55] // Water — darker blue
    } else if height > 165.0 {
        [0.90, 0.92, 0.95] // Snow
    } else if height > 120.0 {
        [0.46, 0.48, 0.50] // Rock
    } else if moisture < 0.3 {
        [0.70, 0.60, 0.36] // Desert
    } else if moisture > 0.62 {
        [0.21, 0.43, 0.23] // Forest
    } else {
        [0.34, 0.52, 0.24] // Grassland
    };

    // Height tint (same formula as the shader)
    let tint = ((height + 40.0) / 260.0).clamp(0.0, 1.0) * 0.08;
    let r = base[0] + (0.75 - base[0]) * tint;
    let g = base[1] + (0.75 - base[1]) * tint;
    let b = base[2] + (0.75 - base[2]) * tint;

    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]
}
