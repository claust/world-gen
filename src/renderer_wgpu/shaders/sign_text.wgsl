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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The sign labels are drawn from a 1-bit bitmap font that ends up only a
    // handful of pixels tall on screen, and there is no MSAA on this pass. With a
    // hard alpha-test the glyph edges are a 1-bit mask that shimmers (whole texels
    // flicker on/off) as the camera pans. Instead we linearly filter the mask and
    // convert its smooth coverage gradient into an anti-aliased alpha using the
    // screen-space rate of change: `fwidth` accounts for both screen axes, so the
    // edge stays ~1px wide and stable up close and softens gracefully (rather than
    // breaking up) as the text is minified or seen at a grazing angle.
    let coverage = textureSample(font_texture, font_sampler, in.uv).r;
    let aa = max(fwidth(coverage), 1e-5);
    let alpha = clamp((coverage - 0.5) / aa + 0.5, 0.0, 1.0);
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
