//! Dev-only MSDF font-atlas baker. Run once to (re)generate the HUD font:
//!
//! ```bash
//! cargo run --example bake_hud_font
//! ```
//!
//! It rasterizes printable ASCII (`0x20..=0x7E`) of `assets/fonts/Inter-Medium.ttf`
//! into a single multi-channel signed distance field (MSDF) atlas and writes three
//! files next to the font:
//!
//! - `hud_atlas.bin`  — brotli-compressed raw RGBA8 texels (shipped; loaded at runtime)
//! - `hud_atlas.json` — per-glyph metrics + atlas header (shipped; `include_str!`)
//! - `hud_atlas.png`  — the same texels as a PNG, for eyeballing only (not shipped)
//!
//! The shipped game never depends on `fdsm`/`nalgebra`: it just decompresses the
//! `.bin` and uploads it. Only this example pulls in the MSDF generator.

use std::io::Write as _;
use std::path::Path;

use fdsm::bezier::scanline::FillRule;
use fdsm::generate::generate_msdf;
use fdsm::render::correct_sign_msdf;
use fdsm::shape::Shape;
use fdsm::transform::Transform;
use fdsm_ttf_parser::load_shape_from_face;
use image::{Rgba, RgbaImage};
use nalgebra::{Affine2, Similarity2, Vector2};
use ttf_parser::Face;

/// Distance-field spread, in atlas texels, on each side of an edge. The runtime
/// shader is told this same value (via the JSON `px_range`) to size its anti-aliasing.
const RANGE: f64 = 4.0;
/// Target on-bake glyph resolution: roughly this many texels per em. Higher = finer
/// curve capture in the atlas (MSDF stays crisp at any *display* size regardless).
const PX_PER_EM: f64 = 40.0;
/// Atlas width; height grows to fit the shelf-packed rows.
const ATLAS_W: u32 = 512;
/// Transparent gap between packed glyphs so neighbours never bleed under filtering.
const PAD: u32 = 2;

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts");
    let ttf = std::fs::read(dir.join("Inter-Medium.ttf")).expect("read Inter-Medium.ttf");
    let face = Face::parse(&ttf, 0).expect("parse font");

    let upm = face.units_per_em() as f64;
    let shrinkage = upm / PX_PER_EM; // font units per atlas texel
    let margin_em = RANGE * shrinkage / upm; // distance-field padding, in em

    // --- Rasterize every glyph into its own MSDF tile ---
    struct Baked {
        ch: char,
        img: Option<RgbaImage>, // None for whitespace (advance only, no quad)
        advance: f32,           // em
        // Plane bounds in em, relative to the pen origin on the baseline. These cover
        // the *padded* image extent (glyph bbox grown by `margin_em`) so the field's
        // 0.5 crossing lands on the true outline when drawn.
        left: f32,
        bottom: f32,
        right: f32,
        top: f32,
    }

    let mut baked: Vec<Baked> = Vec::new();
    for code in 0x20u8..=0x7E {
        let ch = code as char;
        let Some(gid) = face.glyph_index(ch) else {
            continue;
        };
        let advance = face.glyph_hor_advance(gid).unwrap_or(0) as f64 / upm;

        match face.glyph_bounding_box(gid) {
            Some(bbox) => {
                let w = ((bbox.x_max - bbox.x_min) as f64 / shrinkage + 2.0 * RANGE).ceil() as u32;
                let h = ((bbox.y_max - bbox.y_min) as f64 / shrinkage + 2.0 * RANGE).ceil() as u32;

                let Some(mut shape) = load_shape_from_face(&face, gid) else {
                    continue;
                };
                // Map font units → texels (1/shrinkage), then place the glyph bbox at
                // (RANGE, RANGE) so a RANGE-texel margin surrounds it on every side.
                let t: Affine2<f64> = nalgebra::convert(Similarity2::new(
                    Vector2::new(
                        RANGE - bbox.x_min as f64 / shrinkage,
                        RANGE - bbox.y_min as f64 / shrinkage,
                    ),
                    0.0,
                    1.0 / shrinkage,
                ));
                shape.transform(&t);

                let colored = Shape::edge_coloring_simple(shape, 0.03, 69_441_337_420);
                let prepared = colored.prepare();

                // fdsm generates into an RGB image; we render then promote to RGBA.
                let mut rgb = image::RgbImage::new(w, h);
                generate_msdf(&prepared, RANGE, &mut rgb);
                correct_sign_msdf(&mut rgb, &prepared, FillRule::Nonzero);

                // Font +y points up but image rows go down → flip vertically.
                let mut img = RgbaImage::new(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let p = rgb.get_pixel(x, h - 1 - y);
                        img.put_pixel(x, y, Rgba([p[0], p[1], p[2], 255]));
                    }
                }

                baked.push(Baked {
                    ch,
                    img: Some(img),
                    advance: advance as f32,
                    left: (bbox.x_min as f64 / upm - margin_em) as f32,
                    bottom: (bbox.y_min as f64 / upm - margin_em) as f32,
                    right: (bbox.x_max as f64 / upm + margin_em) as f32,
                    top: (bbox.y_max as f64 / upm + margin_em) as f32,
                });
            }
            None => baked.push(Baked {
                ch,
                img: None,
                advance: advance as f32,
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
                top: 0.0,
            }),
        }
    }

    // --- Tabular digits: give 0-9 a uniform advance so live readouts don't jitter ---
    let digit_adv = baked
        .iter()
        .filter(|b| b.ch.is_ascii_digit())
        .map(|b| b.advance)
        .fold(0.0_f32, f32::max);
    for b in baked.iter_mut().filter(|b| b.ch.is_ascii_digit()) {
        b.advance = digit_adv;
    }

    // --- Shelf-pack the tiles into one atlas (tallest first for tight rows) ---
    let mut order: Vec<usize> = (0..baked.len())
        .filter(|&i| baked[i].img.is_some())
        .collect();
    order.sort_by_key(|&i| std::cmp::Reverse(baked[i].img.as_ref().unwrap().height()));

    let mut placements: Vec<(usize, u32, u32)> = Vec::new(); // (index, x, y)
    let (mut pen_x, mut pen_y, mut row_h, mut atlas_h) = (PAD, PAD, 0u32, 0u32);
    for &i in &order {
        let img = baked[i].img.as_ref().unwrap();
        let (gw, gh) = (img.width(), img.height());
        if pen_x + gw + PAD > ATLAS_W {
            pen_x = PAD;
            pen_y += row_h + PAD;
            row_h = 0;
        }
        placements.push((i, pen_x, pen_y));
        pen_x += gw + PAD;
        row_h = row_h.max(gh);
        atlas_h = atlas_h.max(pen_y + gh + PAD);
    }

    let mut atlas = RgbaImage::from_pixel(ATLAS_W, atlas_h, Rgba([0, 0, 0, 0]));
    // uv rects, filled in during blit (index → [u0, v0, u1, v1])
    let mut uvs: Vec<Option<[f32; 4]>> = vec![None; baked.len()];
    for &(i, x, y) in &placements {
        let img = baked[i].img.as_ref().unwrap();
        let (gw, gh) = (img.width(), img.height());
        for gy in 0..gh {
            for gx in 0..gw {
                atlas.put_pixel(x + gx, y + gy, *img.get_pixel(gx, gy));
            }
        }
        uvs[i] = Some([
            x as f32 / ATLAS_W as f32,
            y as f32 / atlas_h as f32,
            (x + gw) as f32 / ATLAS_W as f32,
            (y + gh) as f32 / atlas_h as f32,
        ]);
    }

    // --- Write JSON metrics ---
    let ascender = face.ascender() as f64 / upm;
    let descender = face.descender() as f64 / upm;
    let line_gap = face.line_gap() as f64 / upm;
    let line_height = ascender - descender + line_gap;

    let mut glyphs = String::new();
    for (i, b) in baked.iter().enumerate() {
        let uv = uvs[i].unwrap_or([0.0; 4]);
        let plane = [b.left, b.bottom, b.right, b.top];
        // Escape the two JSON-significant ASCII chars that can appear as a glyph.
        let ch_json = match b.ch {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            c => c.to_string(),
        };
        if i > 0 {
            glyphs.push_str(",\n");
        }
        glyphs.push_str(&format!(
            "    {{ \"ch\": \"{ch_json}\", \"advance\": {:.6}, \"plane\": [{:.6}, {:.6}, {:.6}, {:.6}], \"uv\": [{:.6}, {:.6}, {:.6}, {:.6}] }}",
            b.advance, plane[0], plane[1], plane[2], plane[3], uv[0], uv[1], uv[2], uv[3],
        ));
    }
    let json = format!(
        "{{\n  \"atlas_w\": {ATLAS_W},\n  \"atlas_h\": {atlas_h},\n  \"px_range\": {RANGE:.1},\n  \"line_height\": {line_height:.6},\n  \"ascender\": {ascender:.6},\n  \"glyphs\": [\n{glyphs}\n  ]\n}}\n"
    );
    std::fs::write(dir.join("hud_atlas.json"), &json).expect("write json");

    // --- Write raw RGBA (brotli) + a PNG for inspection ---
    let raw = atlas.as_raw();
    let mut compressed = Vec::new();
    {
        let mut w = brotli::CompressorWriter::new(&mut compressed, 4096, 11, 22);
        w.write_all(raw).expect("brotli write");
    }
    std::fs::write(dir.join("hud_atlas.bin"), &compressed).expect("write bin");
    atlas.save(dir.join("hud_atlas.png")).expect("write png");

    println!(
        "baked {} glyphs → atlas {ATLAS_W}x{atlas_h} ({} KiB raw, {} KiB brotli)",
        baked.len(),
        raw.len() / 1024,
        compressed.len() / 1024,
    );
}
