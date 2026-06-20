struct HudUniform {
    screen_size: vec2<f32>,
    px_range: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> hud: HudUniform;

@group(1) @binding(0) var font_texture: texture_2d<f32>;
@group(1) @binding(1) var font_sampler: sampler;
@group(1) @binding(2) var image_texture: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) sdf: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let ndc_x = (in.position.x / hud.screen_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (in.position.y / hud.screen_size.y) * 2.0;
    out.clip_position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.sdf = in.sdf;
    return out;
}

fn median(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

// SDF shape ids carried in `sdf.x`; must match the constants in hud_pass.rs.
const SDF_DIAMOND: f32 = 1.0;      // |x| + |y| <= 1
const SDF_DIAMOND_SEAM: f32 = 2.0; // diamond that also fades across the y = 0 seam
const SDF_DISC: f32 = 3.0;         // length(p) <= 1
const SDF_RING: f32 = 4.0;         // sdf.y <= length(p) <= 1
const SDF_BOX: f32 = 5.0;          // max(|x|, |y|) <= 1
const SDF_IMAGE: f32 = 6.0;        // sample image_texture at uv (0..1), tinted by color

// Coverage of an SDF inside-negative field, anti-aliased over ~1 screen pixel using the
// field's screen-space gradient `aa`. The gradient is passed in (not taken with `fwidth`
// here) because derivative builtins must run in uniform control flow under WebGPU/Tint,
// and this is called from per-shape branches — see the hoisting note in `fs_main`.
fn sdf_coverage(dist: f32, aa: f32) -> f32 {
    return clamp(0.5 - dist / aa, 0.0, 1.0);
}

// Whether `id` (an `sdf.x` value) names shape `want`. The ids are integers, so a 0.5
// window matches exactly one and lets unknown ids fall through to a transparent default.
fn is_shape(id: f32, want: f32) -> bool {
    return abs(id - want) < 0.5;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // WebGPU/Tint requires derivative builtins (`fwidth`) to run in uniform control
    // flow; the branches below select on the per-vertex `sdf`/`uv` kind, which is
    // non-uniform. So every derivative this shader needs is taken here, before any
    // branch, and the results are fed into the branches. (Native wgpu/naga is lenient
    // and accepts in-branch `fwidth`, which is why this only broke the browser build.)
    let p = in.uv;
    let uv_fw = fwidth(in.uv);
    let d_diamond = abs(p.x) + abs(p.y) - 1.0;
    let r = length(p);
    let d_disc = r - 1.0;
    let d_ring = max(r - 1.0, in.sdf.y - r);
    let d_box = max(abs(p.x), abs(p.y)) - 1.0;
    let aa_diamond = max(fwidth(d_diamond), 1e-5);
    let aa_disc = max(fwidth(d_disc), 1e-5);
    let aa_ring = max(fwidth(d_ring), 1e-5);
    let aa_box = max(fwidth(d_box), 1e-5);
    let aa_seam = max(uv_fw.y, 1e-5);

    // Sample unconditionally (uniform control flow required by WebGPU). Vertex kinds:
    // sdf.x > 0 is an anti-aliased SDF primitive (uv holds its normalized local coord);
    // otherwise solid quads (panel, clock ticks/horizon) carry the sentinel uv == (-1,
    // -1) and everything else is MSDF text (uv >= 0).
    let msd = textureSample(font_texture, font_sampler, max(in.uv, vec2(0.0))).rgb;
    let img = textureSample(image_texture, font_sampler, clamp(in.uv, vec2(0.0), vec2(1.0)));

    if (in.sdf.x > 0.5) {
        let shape = in.sdf.x;

        // Textured backdrop: return the sampled image tinted (and faded) by the vertex
        // color, so the same color channel both dims and sets the backdrop's opacity.
        if (is_shape(shape, SDF_IMAGE)) {
            return vec4<f32>(img.rgb * in.color.rgb, img.a * in.color.a);
        }

        // Unknown ids stay fully transparent, so adding/reordering shape constants can
        // never silently fall through to the wrong primitive.
        var alpha = 0.0;

        if (is_shape(shape, SDF_DIAMOND) || is_shape(shape, SDF_DIAMOND_SEAM)) {
            // Rhombus: boundary |x| + |y| = 1.
            alpha = sdf_coverage(d_diamond, aa_diamond);
            // The seam-fading half additionally fades in across the E–W seam (local
            // y = 0), anti-aliasing the divider where it overlays the layer below.
            if (is_shape(shape, SDF_DIAMOND_SEAM)) {
                alpha *= clamp(0.5 + p.y / aa_seam, 0.0, 1.0);
            }
        } else if (is_shape(shape, SDF_DISC)) {
            // Disc: boundary length(p) = 1.
            alpha = sdf_coverage(d_disc, aa_disc);
        } else if (is_shape(shape, SDF_RING)) {
            // Ring: intersection of the unit disc and the outside of the inner disc,
            // whose normalized radius is sdf.y.
            alpha = sdf_coverage(d_ring, aa_ring);
        } else if (is_shape(shape, SDF_BOX)) {
            // Box: boundary max(|x|, |y|) = 1.
            alpha = sdf_coverage(d_box, aa_box);
        }

        return vec4<f32>(in.color.rgb, in.color.a * alpha);
    }

    if (in.uv.x < 0.0) {
        return in.color;
    }

    // MSDF: the per-channel median is the signed distance to the nearest edge; its
    // 0.5 crossing is the outline. Convert that to an anti-aliased alpha by scaling
    // by the field's spread in *screen* pixels, so the edge stays ~1px wide at any size.
    let sd = median(msd.r, msd.g, msd.b);
    let unit_range = vec2<f32>(hud.px_range) / vec2<f32>(textureDimensions(font_texture));
    let screen_px_range = max(0.5 * dot(unit_range, vec2(1.0) / uv_fw), 1.0);
    let alpha = clamp((sd - 0.5) * screen_px_range + 0.5, 0.0, 1.0);
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
