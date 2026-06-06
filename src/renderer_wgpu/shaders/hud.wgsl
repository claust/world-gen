struct HudUniform {
    screen_size: vec2<f32>,
    px_range: f32,
    _pad: f32,
};

@group(0) @binding(0) var<uniform> hud: HudUniform;

@group(1) @binding(0) var font_texture: texture_2d<f32>;
@group(1) @binding(1) var font_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
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
    let ndc_x = (in.position.x / hud.screen_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (in.position.y / hud.screen_size.y) * 2.0;
    out.clip_position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

fn median(a: f32, b: f32, c: f32) -> f32 {
    return max(min(a, b), min(max(a, b), c));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample unconditionally (uniform control flow required by WebGPU). Solid-color
    // quads (panel, compass, clock) carry a sentinel uv.x < 0; everything else is text.
    let msd = textureSample(font_texture, font_sampler, max(in.uv, vec2(0.0))).rgb;
    if (in.uv.x < 0.0) {
        return in.color;
    }

    // MSDF: the per-channel median is the signed distance to the nearest edge; its
    // 0.5 crossing is the outline. Convert that to an anti-aliased alpha by scaling
    // by the field's spread in *screen* pixels, so the edge stays ~1px wide at any size.
    let sd = median(msd.r, msd.g, msd.b);
    let unit_range = vec2<f32>(hud.px_range) / vec2<f32>(textureDimensions(font_texture));
    let screen_px_range = max(0.5 * dot(unit_range, vec2(1.0) / fwidth(in.uv)), 1.0);
    let alpha = clamp((sd - 0.5) * screen_px_range + 0.5, 0.0, 1.0);
    return vec4<f32>(in.color.rgb, in.color.a * alpha);
}
