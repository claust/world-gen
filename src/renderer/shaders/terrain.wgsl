struct TerrainUniform {
    view_proj: mat4x4<f32>,
    light_direction: vec4<f32>,
    ambient: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> uniforms: TerrainUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) albedo: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = uniforms.view_proj * vec4<f32>(input.position, 1.0);
    out.world_normal = input.normal;
    out.albedo = input.color;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(input.world_normal);
    let l = normalize(uniforms.light_direction.xyz);
    let direct = max(dot(n, l), 0.0);
    let shade = uniforms.ambient.x + direct * 0.82;
    let color = input.albedo * shade;
    return vec4<f32>(color, 1.0);
}
