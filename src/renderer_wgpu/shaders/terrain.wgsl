struct FrameUniform {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    time: vec4<f32>,
};

struct MaterialUniform {
    light_direction: vec4<f32>,
    ambient: vec4<f32>,
    fog_color: vec4<f32>,
    fog_params: vec4<f32>,
    sun_color: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniform;
@group(1) @binding(0) var<uniform> material: MaterialUniform;
@group(2) @binding(0) var terrain_atlas: texture_2d<f32>;
@group(2) @binding(1) var terrain_sampler: sampler;

const TEXTURE_SCALE: f32 = 0.1;
const TILE_COUNT: f32 = 5.0;
const HALF_TEXEL: f32 = 0.5 / 128.0;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) biome_data: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) biome_data: vec3<f32>,
    @location(2) world_position: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = frame.view_proj * vec4<f32>(input.position, 1.0);
    out.world_normal = input.normal;
    out.biome_data = input.biome_data;
    out.world_position = input.position;
    return out;
}

fn sample_biome(biome_id: f32, world_pos: vec3<f32>) -> vec3<f32> {
    let tile = round(biome_id);
    let uv_local = fract(world_pos.xz * TEXTURE_SCALE);

    // Inset UV to prevent bleeding between atlas tiles
    let inset_u = HALF_TEXEL + uv_local.x * (1.0 - 2.0 * HALF_TEXEL);
    let atlas_u = (tile + inset_u) / TILE_COUNT;
    let atlas_v = HALF_TEXEL + uv_local.y * (1.0 - 2.0 * HALF_TEXEL);

    return textureSample(terrain_atlas, terrain_sampler, vec2<f32>(atlas_u, atlas_v)).rgb;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let biome_a = input.biome_data.x;
    let biome_b = input.biome_data.y;
    let blend_factor = input.biome_data.z;

    let color_a = sample_biome(biome_a, input.world_position);
    let color_b = sample_biome(biome_b, input.world_position);
    let albedo = mix(color_a, color_b, blend_factor);

    let n = normalize(input.world_normal);
    let l = normalize(material.light_direction.xyz);
    let direct = max(dot(n, l), 0.0);
    let color = albedo * material.ambient.x + albedo * direct * 0.82 * material.sun_color.rgb;

    let dist = distance(input.world_position, frame.camera_position.xyz);
    let fog_start = material.fog_params.x;
    let fog_end = material.fog_params.y;
    let fog_factor = clamp((dist - fog_start) / (fog_end - fog_start), 0.0, 1.0);
    let final_color = mix(color, material.fog_color.rgb, fog_factor);

    return vec4<f32>(final_color, 1.0);
}
