// Terrain shader: heightmap-based biome-atlas rendering with hemisphere
// ambient (from lighting.wgsl, concatenated ahead of this file) and
// single-cascade sun shadow mapping.

struct FrameUniform {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    time: vec4<f32>,
    shadow_params: vec4<f32>,
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
// Terrain uses group 2 for the biome atlas, so the shadow map lives at group 3.
@group(3) @binding(0) var shadow_map: texture_depth_2d;
@group(3) @binding(1) var shadow_sampler: sampler_comparison;

const TEXTURE_SCALE: f32 = 0.1;
const TILE_COUNT: f32 = 5.0;
const TILE_SIZE: f32 = 128.0;
const HALF_TEXEL: f32 = 0.5 / TILE_SIZE;

// 3x3 PCF shadow lookup. Returns 1.0 (fully lit) when outside the shadow
// frustum or when shadows are disabled (night).
fn sample_shadow(world_pos: vec3<f32>, n_dot_l: f32) -> f32 {
    if (frame.shadow_params.w < 0.5) { return 1.0; }
    let lp = frame.light_view_proj * vec4<f32>(world_pos, 1.0);
    let proj = lp.xyz / lp.w;
    let uv = vec2<f32>(proj.x * 0.5 + 0.5, proj.y * -0.5 + 0.5);
    let outside = uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || proj.z > 1.0 || proj.z < 0.0;
    let bias = max(frame.shadow_params.y * (1.0 - n_dot_l), frame.shadow_params.y * 0.2);
    let texel = frame.shadow_params.x;
    let cuv = clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0));
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let off = vec2<f32>(f32(dx), f32(dy)) * texel;
            sum = sum + textureSampleCompare(shadow_map, shadow_sampler, cuv + off, proj.z - bias);
        }
    }
    return select(sum / 9.0, 1.0, outside);
}

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) biome_data: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) @interpolate(flat) biome_ids: vec2<f32>,
    @location(2) blend_factor: f32,
    @location(3) world_position: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = frame.view_proj * vec4<f32>(input.position, 1.0);
    out.world_normal = input.normal;
    out.biome_ids = input.biome_data.xy;
    out.blend_factor = input.biome_data.z;
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
    let biome_a = input.biome_ids.x;
    let biome_b = input.biome_ids.y;
    let blend_factor = input.blend_factor;

    let color_a = sample_biome(biome_a, input.world_position);
    let color_b = sample_biome(biome_b, input.world_position);
    let albedo = mix(color_a, color_b, blend_factor);

    let n = normalize(input.world_normal);
    let l = normalize(material.light_direction.xyz);
    let direct = max(dot(n, l), 0.0);
    let shadow = sample_shadow(input.world_position, direct);
    let color = albedo * hemisphere_ambient(n)
        + albedo * direct * shadow * 0.82 * material.sun_color.rgb;

    let dist = distance(input.world_position, frame.camera_position.xyz);
    let fog_start = material.fog_params.x;
    let fog_end = material.fog_params.y;
    let fog_factor = clamp((dist - fog_start) / (fog_end - fog_start), 0.0, 1.0);
    let final_color = mix(color, material.fog_color.rgb, fog_factor);

    return vec4<f32>(final_color, 1.0);
}
