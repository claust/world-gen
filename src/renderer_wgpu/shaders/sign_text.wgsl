// World-space text painted onto the debug tile-marker sign boards. Geometry is
// pre-transformed into world space on the CPU (one static buffer per chunk); the
// vertex stage projects it camera-relative (see `view_proj_no_translation`) so the
// thin text stays precisely in front of its board far from the world origin.

struct FrameUniform {
    view_proj: mat4x4<f32>,
    inv_view_proj_no_translation: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    time: vec4<f32>,
    shadow_params: vec4<f32>,
    view_proj_no_translation: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniform;

@group(1) @binding(0) var font_texture: texture_2d<f32>;
@group(1) @binding(1) var font_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position =
        frame.view_proj_no_translation * vec4<f32>(in.position - frame.camera_position.xyz, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

// MSDF distance-field spread, in atlas texels. Must match the `RANGE` the atlas was
// baked with (see `examples/bake_hud_font.rs` / `hud_atlas.json`'s `px_range`).
const PX_RANGE: f32 = 4.0;

fn median(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Reconstruct the glyph edge from the multi-channel signed distance field. The
    // per-channel median is the signed distance; its 0.5 crossing is the outline.
    // Scaling by the field's spread in *screen* pixels (via `fwidth`) keeps the edge
    // ~1px wide and stable both up close and when the sign is minified or grazing.
    let msd = textureSample(font_texture, font_sampler, in.uv).rgb;
    let sd = median(msd.r, msd.g, msd.b);
    let unit_range = vec2<f32>(PX_RANGE) / vec2<f32>(textureDimensions(font_texture));
    let screen_px_range = max(0.5 * dot(unit_range, vec2(1.0) / fwidth(in.uv)), 1.0);
    let alpha = clamp((sd - 0.5) * screen_px_range + 0.5, 0.0, 1.0);
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
