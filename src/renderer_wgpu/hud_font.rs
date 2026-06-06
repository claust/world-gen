//! HUD/sign font: a baked multi-channel signed distance field (MSDF) atlas.
//!
//! The atlas and per-glyph metrics are produced offline by `examples/bake_hud_font.rs`
//! (Inter, printable ASCII `0x20..=0x7E`) and shipped under `assets/fonts/`:
//!
//! - `hud_atlas.bin`  — brotli-compressed raw RGBA8 texels (the MSDF lives in RGB)
//! - `hud_atlas.json` — atlas header + per-glyph advance / plane bounds / uv rect
//!
//! At runtime we decompress the texels and parse the metrics once into [`MsdfFont`].
//! The fragment shaders reconstruct crisp, resolution-independent edges from the
//! distance field, so text stays sharp at any scale (see `shaders/hud.wgsl`).

use std::io::Read as _;

use bytemuck::{Pod, Zeroable};
use serde::Deserialize;

#[repr(C)]
#[derive(Clone, Copy, Debug, Zeroable, Pod)]
pub struct HudVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl HudVertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Self>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 8,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 16,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x4,
            },
        ],
    };
}

/// Per-glyph layout data, all distances in em (1 em == one font size in pixels).
#[derive(Clone, Copy)]
pub struct Glyph {
    /// Horizontal pen advance.
    pub advance: f32,
    /// Quad extent relative to the pen origin on the baseline, y-up:
    /// `[left, bottom, right, top]`. Covers the distance-field padding, so the
    /// field's 0.5 crossing lands on the true outline. Zero-area for whitespace.
    pub plane: [f32; 4],
    /// Atlas texture rect `[u0, v0, u1, v1]` (top-left, bottom-right).
    pub uv: [f32; 4],
}

impl Glyph {
    /// Whitespace and other glyphs that have no visible quad.
    pub fn is_blank(&self) -> bool {
        self.plane[2] <= self.plane[0]
    }
}

const FIRST_CH: usize = 0x20;
const LAST_CH: usize = 0x7E;
const GLYPH_COUNT: usize = LAST_CH - FIRST_CH + 1;

/// A loaded MSDF font: decompressed atlas texels plus per-glyph metrics.
pub struct MsdfFont {
    pub atlas_w: u32,
    pub atlas_h: u32,
    /// Distance-field spread in atlas texels — the shader uses this to size its AA.
    pub px_range: f32,
    /// Baseline-to-baseline line advance, in em.
    pub line_height: f32,
    /// Baseline-to-top ascent, in em (a line's pen origin sits this far below its top).
    pub ascender: f32,
    glyphs: [Option<Glyph>; GLYPH_COUNT],
}

#[derive(Deserialize)]
struct RawGlyph {
    ch: String,
    advance: f32,
    plane: [f32; 4],
    uv: [f32; 4],
}

#[derive(Deserialize)]
struct RawAtlas {
    atlas_w: u32,
    atlas_h: u32,
    px_range: f32,
    line_height: f32,
    ascender: f32,
    glyphs: Vec<RawGlyph>,
}

impl MsdfFont {
    /// Decompress the baked atlas and parse its metrics. Cheap (one ~80 KiB brotli
    /// blob); call once at startup. Returns the metrics plus the raw RGBA8 texels —
    /// the caller uploads the texels to a texture and lets them drop, so the
    /// half-megabyte CPU copy isn't kept resident for the session.
    pub fn load() -> (Self, Vec<u8>) {
        let raw: RawAtlas = serde_json::from_str(include_str!("../../assets/fonts/hud_atlas.json"))
            .expect("parse hud_atlas.json");

        let expected = raw.atlas_w as usize * raw.atlas_h as usize * 4;
        let compressed: &[u8] = include_bytes!("../../assets/fonts/hud_atlas.bin");
        let mut atlas_rgba = Vec::with_capacity(expected);
        // Bound the read to one byte past the expected size so a corrupt or swapped
        // asset fails the assert below instead of decompressing unboundedly.
        brotli::Decompressor::new(compressed, 4096)
            .take(expected as u64 + 1)
            .read_to_end(&mut atlas_rgba)
            .expect("decompress hud_atlas.bin");
        assert_eq!(
            atlas_rgba.len(),
            expected,
            "hud_atlas.bin does not match hud_atlas.json dimensions — re-run `cargo run --example bake_hud_font`",
        );

        let mut glyphs = [None; GLYPH_COUNT];
        for g in raw.glyphs {
            let Some(c) = g.ch.chars().next() else {
                continue;
            };
            let code = c as usize;
            if (FIRST_CH..=LAST_CH).contains(&code) {
                glyphs[code - FIRST_CH] = Some(Glyph {
                    advance: g.advance,
                    plane: g.plane,
                    uv: g.uv,
                });
            }
        }

        let font = Self {
            atlas_w: raw.atlas_w,
            atlas_h: raw.atlas_h,
            px_range: raw.px_range,
            line_height: raw.line_height,
            ascender: raw.ascender,
            glyphs,
        };
        (font, atlas_rgba)
    }

    /// Glyph metrics for `c`, or `None` outside the baked ASCII range.
    pub fn glyph(&self, c: char) -> Option<Glyph> {
        let code = c as usize;
        if (FIRST_CH..=LAST_CH).contains(&code) {
            self.glyphs[code - FIRST_CH]
        } else {
            None
        }
    }

    /// Rendered width of `text` at font size `px` (em → pixels), in pixels.
    pub fn measure(&self, text: &str, px: f32) -> f32 {
        text.chars()
            .filter_map(|c| self.glyph(c))
            .map(|g| g.advance)
            .sum::<f32>()
            * px
    }
}

/// Append textured quads (6 verts/glyph) for `text` at font size `px`, with the
/// line's top-left corner at `(x, y)` in screen pixels (y down).
pub fn build_text_quads(
    font: &MsdfFont,
    text: &str,
    x: f32,
    y: f32,
    px: f32,
    color: [f32; 4],
    out: &mut Vec<HudVertex>,
) {
    let baseline = y + font.ascender * px;
    let mut pen_x = x;

    for c in text.chars() {
        let Some(g) = font.glyph(c) else {
            continue;
        };
        if g.is_blank() {
            pen_x += g.advance * px;
            continue;
        }

        // Plane is y-up relative to the baseline; screen y grows downward.
        let x0 = pen_x + g.plane[0] * px;
        let x1 = pen_x + g.plane[2] * px;
        let y_top = baseline - g.plane[3] * px;
        let y_bot = baseline - g.plane[1] * px;
        let [u0, v0, u1, v1] = g.uv;

        out.extend_from_slice(&[
            HudVertex {
                position: [x0, y_top],
                uv: [u0, v0],
                color,
            },
            HudVertex {
                position: [x1, y_top],
                uv: [u1, v0],
                color,
            },
            HudVertex {
                position: [x0, y_bot],
                uv: [u0, v1],
                color,
            },
            HudVertex {
                position: [x0, y_bot],
                uv: [u0, v1],
                color,
            },
            HudVertex {
                position: [x1, y_top],
                uv: [u1, v0],
                color,
            },
            HudVertex {
                position: [x1, y_bot],
                uv: [u1, v1],
                color,
            },
        ]);

        pen_x += g.advance * px;
    }
}
