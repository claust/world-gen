//! Old-school, hypsometric world map.
//!
//! The loading screen paints a live one-cell-per-chunk biome map straight from
//! [`crate::world_runtime::GenerationProgress`] (256×256 flat squares). That's
//! fine while the world is being born, but it looks blocky as the static in-game
//! map overlay (the `M` key).
//!
//! This module rebuilds that map once, after generation, by resampling the
//! retained continuous [`Heightmap`] at a much higher resolution. Land is
//! colored by a smooth elevation ramp (the classic physical-atlas hypsometric
//! tint: green lowlands → tan → brown → pale peaks) rather than discrete biome
//! patches, the sea is a single flat color, gentle hillshade adds relief, and a
//! light blur gives the soft, printed-map feel of an old-school atlas.

#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

use crate::world_core::chunk::{CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS};
use crate::world_core::heightmap::Heightmap;

/// Side length (in texels) of the rendered world-map texture. 1024² resamples
/// the world ~4× finer than the per-chunk grid (256²) in each axis, which is
/// enough to dissolve the chunk squares while staying a cheap one-time build.
pub const WORLD_MAP_RES: usize = 1024;

/// Vertical exaggeration applied to the height gradient before shading. The
/// world's height range is small relative to its 65 km span, so the relief needs
/// amplifying to read at map scale.
const RELIEF_EXAGGERATION: f32 = 2.5;

/// Radius (in texels) of the height pre-smoothing. Smooths the heightmap's fine
/// detail-noise so coastlines and elevation bands read as clean, smooth curves
/// rather than feathery noise — the look of a drawn map.
const HEIGHT_BLUR_RADIUS: usize = 4;

/// Radius (in texels) of the final color softening blur. Small, since the height
/// field is already smoothed; just takes the edge off the elevation bands.
const BLUR_RADIUS: usize = 2;

/// Single flat sea color (muted atlas blue). The whole sea reads as one tone.
const SEA_COLOR: [f32; 3] = [0.38, 0.50, 0.60];

/// Hypsometric color ramp: `(t, rgb)` stops from coast (`t = 0`) to the highest
/// land (`t = 1`), in the muted, earthy palette of an old physical map.
const LAND_RAMP: [(f32, [f32; 3]); 6] = [
    (0.00, [0.55, 0.59, 0.45]), // coastal lowland — muted green
    (0.18, [0.66, 0.66, 0.48]), // green-yellow plains
    (0.40, [0.76, 0.72, 0.53]), // khaki uplands
    (0.60, [0.72, 0.61, 0.45]), // tan foothills
    (0.80, [0.60, 0.49, 0.39]), // brown mountains
    (1.00, [0.86, 0.85, 0.83]), // pale rock / snow peaks
];

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Sample the hypsometric ramp at normalized land elevation `t` in `[0, 1]`.
fn ramp_color(t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    for pair in LAND_RAMP.windows(2) {
        let (t0, c0) = pair[0];
        let (t1, c1) = pair[1];
        if t <= t1 {
            let k = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
            return [
                lerp(c0[0], c1[0], k),
                lerp(c0[1], c1[1], k),
                lerp(c0[2], c1[2], k),
            ];
        }
    }
    LAND_RAMP[LAND_RAMP.len() - 1].1
}

/// Separable box blur over a scalar grid (`res`×`res`), clamping at the edges.
fn box_blur_scalar(src: &[f32], res: usize, radius: usize) -> Vec<f32> {
    if radius == 0 {
        return src.to_vec();
    }
    let r = radius as i32;
    let n = res as i32;
    let w = (radius * 2 + 1) as f32;
    let blur_axis = |input: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.0f32; res * res];
        for row in 0..res {
            for col in 0..res {
                let mut acc = 0.0f32;
                for k in -r..=r {
                    let (sr, sc) = if horizontal {
                        (row as i32, (col as i32 + k).clamp(0, n - 1))
                    } else {
                        ((row as i32 + k).clamp(0, n - 1), col as i32)
                    };
                    acc += input[sr as usize * res + sc as usize];
                }
                out[row * res + col] = acc / w;
            }
        }
        out
    };
    let tmp = blur_axis(src, true);
    blur_axis(&tmp, false)
}

/// Separable box blur over an RGB grid (`res`×`res`), clamping at the edges.
fn box_blur(src: &[[f32; 3]], res: usize, radius: usize) -> Vec<[f32; 3]> {
    if radius == 0 {
        return src.to_vec();
    }
    let r = radius as i32;
    let n = res as i32;
    let w = (radius * 2 + 1) as f32;
    let blur_axis = |input: &[[f32; 3]], horizontal: bool| -> Vec<[f32; 3]> {
        let mut out = vec![[0.0f32; 3]; res * res];
        for row in 0..res {
            for col in 0..res {
                let mut acc = [0.0f32; 3];
                for k in -r..=r {
                    let (sr, sc) = if horizontal {
                        (row as i32, (col as i32 + k).clamp(0, n - 1))
                    } else {
                        ((row as i32 + k).clamp(0, n - 1), col as i32)
                    };
                    let p = input[sr as usize * res + sc as usize];
                    acc[0] += p[0];
                    acc[1] += p[1];
                    acc[2] += p[2];
                }
                out[row * res + col] = [acc[0] / w, acc[1] / w, acc[2] / w];
            }
        }
        out
    };
    let tmp = blur_axis(src, true);
    blur_axis(&tmp, false)
}

/// Render an old-school hypsometric world-map image (RGBA8, `res`×`res`,
/// row-major).
///
/// Texel layout matches the loading/minimap convention so the existing overlay
/// UV mapping in `render_map_ui` stays valid: **column = world +x**, **row =
/// world +z**.
pub fn render_world_map(heightmap: &Heightmap, sea_level: f32, res: usize) -> Vec<u8> {
    let world_size_m = WORLD_SIZE_CHUNKS as f32 * CHUNK_SIZE_METERS;
    let step = world_size_m / res as f32;

    // Pass 1: sample the continuous height field at every texel center. Done up
    // front (and in parallel) so the shading pass can read neighbor heights for
    // the relief gradient without re-sampling the noise.
    let sample_row = |row: usize| -> Vec<f32> {
        let wz = (row as f32 + 0.5) * step;
        (0..res)
            .map(|col| {
                let wx = (col as f32 + 0.5) * step;
                heightmap.sample_height(wx, wz)
            })
            .collect()
    };
    #[cfg(not(target_arch = "wasm32"))]
    let raw_heights: Vec<f32> = (0..res).into_par_iter().flat_map(sample_row).collect();
    #[cfg(target_arch = "wasm32")]
    let raw_heights: Vec<f32> = (0..res).flat_map(sample_row).collect();

    // Smooth the height field so coastlines and elevation bands become clean
    // curves instead of the heightmap's feathery detail-noise.
    let heights = box_blur_scalar(&raw_heights, res, HEIGHT_BLUR_RADIUS);

    // Normalize land elevation against the actual tallest land so the ramp
    // spreads across the world's real height range whatever the seed.
    let max_land = heights
        .iter()
        .copied()
        .filter(|&h| h >= sea_level)
        .fold(sea_level, f32::max);
    let land_span = (max_land - sea_level).max(1.0);

    // Light from the upper-left (north-west), the cartographic convention for
    // relief shading.
    let light = glam::Vec3::new(-0.6, 1.0, -0.6).normalize();
    let at = |r: usize, c: usize| heights[r * res + c];

    // Pass 2: build the (pre-blur) color grid — flat sea, hypsometric land tint,
    // gentle hillshade.
    let color_row = |row: usize| -> Vec<[f32; 3]> {
        let rn = row.saturating_sub(1);
        let rs = (row + 1).min(res - 1);
        (0..res)
            .map(|col| {
                let h = at(row, col);
                if h < sea_level {
                    return SEA_COLOR;
                }
                // Slight gamma gives the lowlands more of the ramp, like a real
                // physical map where most land sits low.
                let t = ((h - sea_level) / land_span).clamp(0.0, 1.0).powf(0.85);
                let mut color = ramp_color(t);

                let cw = col.saturating_sub(1);
                let ce = (col + 1).min(res - 1);
                let dhx = (at(row, ce) - at(row, cw)) / (2.0 * step);
                let dhz = (at(rs, col) - at(rn, col)) / (2.0 * step);
                let normal =
                    glam::Vec3::new(-dhx * RELIEF_EXAGGERATION, 1.0, -dhz * RELIEF_EXAGGERATION)
                        .normalize();
                // Gentle relief: a high ambient floor keeps the tint close to its
                // true ramp color, so slopes only nudge it lighter/darker.
                let shade = (0.82 + 0.30 * normal.dot(light).max(0.0)).clamp(0.7, 1.12);
                color[0] *= shade;
                color[1] *= shade;
                color[2] *= shade;
                color
            })
            .collect()
    };
    #[cfg(not(target_arch = "wasm32"))]
    let colors: Vec<[f32; 3]> = (0..res).into_par_iter().flat_map(color_row).collect();
    #[cfg(target_arch = "wasm32")]
    let colors: Vec<[f32; 3]> = (0..res).flat_map(color_row).collect();

    // Pass 3: soften everything (elevation bands + coastline) into the smooth
    // gradients of a printed map.
    let blurred = box_blur(&colors, res, BLUR_RADIUS);

    // Pass 4: quantize to RGBA8.
    let mut pixels = vec![0u8; res * res * 4];
    for (i, c) in blurred.iter().enumerate() {
        let o = i * 4;
        pixels[o] = (c[0].clamp(0.0, 1.0) * 255.0) as u8;
        pixels[o + 1] = (c[1].clamp(0.0, 1.0) * 255.0) as u8;
        pixels[o + 2] = (c[2].clamp(0.0, 1.0) * 255.0) as u8;
        pixels[o + 3] = 255;
    }
    pixels
}
