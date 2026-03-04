struct BlurParams {
    direction: vec2<f32>,
    texel_size: vec2<f32>,
};

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BlurParams;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) id: u32) -> VertexOutput {
    // Fullscreen triangle: 3 vertices covering entire screen
    let uv = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
    var out: VertexOutput;
    out.clip_position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_blur(input: VertexOutput) -> @location(0) vec4<f32> {
    // 9-tap Gaussian kernel (sigma ~3)
    let w0 = 0.227027;
    let w1 = 0.1945946;
    let w2 = 0.1216216;
    let w3 = 0.054054;
    let w4 = 0.016216;

    let step = params.direction * params.texel_size;

    var color = textureSample(input_tex, input_sampler, input.uv) * w0;
    color += textureSample(input_tex, input_sampler, input.uv + step * 1.0) * w1;
    color += textureSample(input_tex, input_sampler, input.uv - step * 1.0) * w1;
    color += textureSample(input_tex, input_sampler, input.uv + step * 2.0) * w2;
    color += textureSample(input_tex, input_sampler, input.uv - step * 2.0) * w2;
    color += textureSample(input_tex, input_sampler, input.uv + step * 3.0) * w3;
    color += textureSample(input_tex, input_sampler, input.uv - step * 3.0) * w3;
    color += textureSample(input_tex, input_sampler, input.uv + step * 4.0) * w4;
    color += textureSample(input_tex, input_sampler, input.uv - step * 4.0) * w4;

    return color;
}

@fragment
fn fs_blit(input: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(input_tex, input_sampler, input.uv);
}
