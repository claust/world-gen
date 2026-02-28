use bytemuck::{Pod, Zeroable};

pub const GLYPH_W: u32 = 8;
pub const GLYPH_H: u32 = 12;
pub const ATLAS_COLS: u32 = 32;
pub const ATLAS_W: u32 = ATLAS_COLS * GLYPH_W; // 256
pub const ATLAS_H: u32 = GLYPH_H; // 12

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

/// Maps a character to its index in the glyph atlas (0..31), or None.
fn char_index(c: char) -> Option<u32> {
    match c {
        '0'..='9' => Some(c as u32 - '0' as u32),
        '.' => Some(10),
        '-' => Some(11),
        ':' => Some(12),
        'X' => Some(13),
        'Y' => Some(14),
        'Z' => Some(15),
        'N' => Some(16),
        'E' => Some(17),
        'S' => Some(18),
        'W' => Some(19),
        'm' => Some(20),
        ' ' => Some(21),
        _ => None,
    }
}

/// Returns (u_left, v_top, u_right, v_bottom) for a character glyph.
pub fn glyph_uv(c: char) -> Option<(f32, f32, f32, f32)> {
    let idx = char_index(c)?;
    let u0 = (idx * GLYPH_W) as f32 / ATLAS_W as f32;
    let u1 = ((idx + 1) * GLYPH_W) as f32 / ATLAS_W as f32;
    Some((u0, 0.0, u1, 1.0))
}

/// Appends 6 vertices (2 triangles) per character to `out`.
pub fn build_text_quads(
    text: &str,
    x: f32,
    y: f32,
    scale: f32,
    color: [f32; 4],
    out: &mut Vec<HudVertex>,
) {
    let gw = GLYPH_W as f32 * scale;
    let gh = GLYPH_H as f32 * scale;

    for (i, c) in text.chars().enumerate() {
        let Some((u0, v0, u1, v1)) = glyph_uv(c) else {
            continue;
        };
        let px = x + i as f32 * gw;
        let py = y;

        // Two triangles: top-left, top-right, bottom-left, then bottom-left, top-right, bottom-right
        out.extend_from_slice(&[
            HudVertex {
                position: [px, py],
                uv: [u0, v0],
                color,
            },
            HudVertex {
                position: [px + gw, py],
                uv: [u1, v0],
                color,
            },
            HudVertex {
                position: [px, py + gh],
                uv: [u0, v1],
                color,
            },
            HudVertex {
                position: [px, py + gh],
                uv: [u0, v1],
                color,
            },
            HudVertex {
                position: [px + gw, py],
                uv: [u1, v0],
                color,
            },
            HudVertex {
                position: [px + gw, py + gh],
                uv: [u1, v1],
                color,
            },
        ]);
    }
}

// 8×12 bitmap glyph data. Each glyph is 12 bytes (one per row), MSB = leftmost pixel.
type Glyph = [u8; 12];

#[rustfmt::skip]
const GLYPHS: [Glyph; 22] = [
    // '0'
    [0x00, 0x00, 0x7C, 0xC6, 0xCE, 0xD6, 0xE6, 0xC6, 0xC6, 0x7C, 0x00, 0x00],
    // '1'
    [0x00, 0x00, 0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00, 0x00],
    // '2'
    [0x00, 0x00, 0x7C, 0xC6, 0x06, 0x0C, 0x18, 0x30, 0x66, 0xFE, 0x00, 0x00],
    // '3'
    [0x00, 0x00, 0x7C, 0xC6, 0x06, 0x3C, 0x06, 0x06, 0xC6, 0x7C, 0x00, 0x00],
    // '4'
    [0x00, 0x00, 0x0C, 0x1C, 0x3C, 0x6C, 0xCC, 0xFE, 0x0C, 0x0C, 0x00, 0x00],
    // '5'
    [0x00, 0x00, 0xFE, 0xC0, 0xC0, 0xFC, 0x06, 0x06, 0xC6, 0x7C, 0x00, 0x00],
    // '6'
    [0x00, 0x00, 0x38, 0x60, 0xC0, 0xFC, 0xC6, 0xC6, 0xC6, 0x7C, 0x00, 0x00],
    // '7'
    [0x00, 0x00, 0xFE, 0xC6, 0x06, 0x0C, 0x18, 0x30, 0x30, 0x30, 0x00, 0x00],
    // '8'
    [0x00, 0x00, 0x7C, 0xC6, 0xC6, 0x7C, 0xC6, 0xC6, 0xC6, 0x7C, 0x00, 0x00],
    // '9'
    [0x00, 0x00, 0x7C, 0xC6, 0xC6, 0x7E, 0x06, 0x06, 0x0C, 0x78, 0x00, 0x00],
    // '.'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00],
    // '-'
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x7E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
    // ':'
    [0x00, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00, 0x00, 0x00],
    // 'X'
    [0x00, 0x00, 0xC6, 0xC6, 0x6C, 0x38, 0x38, 0x6C, 0xC6, 0xC6, 0x00, 0x00],
    // 'Y'
    [0x00, 0x00, 0xC6, 0xC6, 0x6C, 0x38, 0x18, 0x18, 0x18, 0x18, 0x00, 0x00],
    // 'Z'
    [0x00, 0x00, 0xFE, 0x06, 0x0C, 0x18, 0x30, 0x60, 0xC0, 0xFE, 0x00, 0x00],
    // 'N'
    [0x00, 0x00, 0xC6, 0xE6, 0xF6, 0xDE, 0xCE, 0xC6, 0xC6, 0xC6, 0x00, 0x00],
    // 'E'
    [0x00, 0x00, 0xFE, 0xC0, 0xC0, 0xFC, 0xC0, 0xC0, 0xC0, 0xFE, 0x00, 0x00],
    // 'S'
    [0x00, 0x00, 0x7C, 0xC6, 0xC0, 0x7C, 0x06, 0x06, 0xC6, 0x7C, 0x00, 0x00],
    // 'W'
    [0x00, 0x00, 0xC6, 0xC6, 0xC6, 0xD6, 0xD6, 0xFE, 0xEE, 0xC6, 0x00, 0x00],
    // 'm'
    [0x00, 0x00, 0x00, 0x00, 0x6C, 0xFE, 0xD6, 0xD6, 0xD6, 0xC6, 0x00, 0x00],
    // ' '
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
];

/// Generates the raw R8 pixel data for the 256×12 font atlas.
pub fn generate_atlas_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; (ATLAS_W * ATLAS_H) as usize];

    for (glyph_idx, glyph) in GLYPHS.iter().enumerate() {
        let base_x = glyph_idx as u32 * GLYPH_W;
        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..GLYPH_W {
                let on = (bits >> (7 - col)) & 1;
                let px = base_x + col;
                let py = row as u32;
                pixels[(py * ATLAS_W + px) as usize] = on * 255;
            }
        }
    }

    pixels
}
