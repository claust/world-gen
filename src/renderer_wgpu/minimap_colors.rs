use crate::world_core::chunk::SEA_LEVEL;

/// Approximate biome colors for the minimap display.
/// Uses hard thresholds (not the smooth blending from `biome_blend()` in the terrain shader)
/// since the minimap doesn't need per-texel accuracy.
pub fn biome_color_rgba(height: f32, moisture: f32) -> [u8; 4] {
    let base: [f32; 3] = if height < SEA_LEVEL {
        [0.15, 0.30, 0.55] // Water — darker blue
    } else if height > 165.0 {
        [0.88, 0.90, 0.93] // Snow — near-white with slight blue tint
    } else if height > 120.0 {
        [0.42, 0.42, 0.43] // Rock — gray
    } else if moisture < 0.3 {
        [0.68, 0.58, 0.34] // Desert — tan
    } else if moisture > 0.62 {
        [0.18, 0.38, 0.18] // Forest — dark green
    } else {
        [0.30, 0.48, 0.20] // Grassland — green
    };

    // Height tint
    let tint = ((height + 40.0) / 260.0).clamp(0.0, 1.0) * 0.08;
    let r = base[0] + (0.75 - base[0]) * tint;
    let g = base[1] + (0.75 - base[1]) * tint;
    let b = base[2] + (0.75 - base[2]) * tint;

    [(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]
}
