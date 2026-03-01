struct HudUniform {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> hud: HudUniform;

@group(1) @binding(0) var map_texture: texture_2d<f32>;
@group(1) @binding(1) var map_sampler: sampler;

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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Solid-color vertices use negative UV as sentinel
    if (in.uv.x < 0.0) {
        return in.color;
    }
    let tex = textureSample(map_texture, map_sampler, in.uv);
    return vec4<f32>(tex.rgb, tex.a * in.color.a);
}
