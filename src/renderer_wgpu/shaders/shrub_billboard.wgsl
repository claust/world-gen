// Shrub billboard shader.
//
// Renders crossed/capped quads (see `shrub_billboard_mesh`) as alpha-tested
// foliage cards. The silhouette is a procedural bush-blob mask derived from the
// quad UV (a radial falloff with a little angular noise) — no textures. Verts
// carry up-biased hemisphere normals so the day/night lighting rounds the card
// like a real bush instead of reading as flat cardboard.
//
// The mesh reuses the standard instanced `Vertex` layout; the per-vertex UV is
// packed into the colour attribute's xy channel.

struct FrameUniform {
    view_proj: mat4x4<f32>,
    inv_view_proj_no_translation: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    time: vec4<f32>,
    shadow_params: vec4<f32>,
    view_proj_no_translation: mat4x4<f32>,
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

struct VertexInput {
    // Per-vertex (slot 0)
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv_packed: vec3<f32>, // .xy = quad UV, .z unused

    // Per-instance (slot 1)
    @location(3) inst_position: vec3<f32>,
    @location(4) inst_rotation_y: f32,
    @location(5) inst_scale: vec3<f32>,
    @location(6) inst_color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) tint: vec3<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let c = cos(input.inst_rotation_y);
    let s = sin(input.inst_rotation_y);

    // Scale, then rotate around Y, then translate (same convention as instanced.wgsl).
    let scaled = input.position * input.inst_scale;
    let rotated = vec3<f32>(
        scaled.x * c - scaled.z * s,
        scaled.y,
        scaled.x * s + scaled.z * c,
    );
    let world_pos = rotated + input.inst_position;

    let rot_normal = vec3<f32>(
        input.normal.x * c - input.normal.z * s,
        input.normal.y,
        input.normal.x * s + input.normal.z * c,
    );

    var out: VertexOutput;
    out.clip_position =
        frame.view_proj_no_translation * vec4<f32>(world_pos - frame.camera_position.xyz, 1.0);
    out.world_normal = rot_normal;
    out.tint = input.inst_color.rgb;
    out.world_position = world_pos;
    out.uv = input.uv_packed.xy;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Procedural bush-blob silhouette. Centre the UV, take the radial distance,
    // and trim everything past a noisy radius so the card reads as a ragged blob
    // rather than a hard rectangle.
    let centered = input.uv * 2.0 - vec2<f32>(1.0, 1.0);
    let r = length(centered);
    let angle = atan2(centered.y, centered.x);
    // A couple of harmonics give the rim some raggedness without textures.
    let wobble = 0.10 * sin(angle * 6.0) + 0.06 * sin(angle * 13.0 + 1.7);
    let threshold = 0.92 + wobble;
    // smoothstep requires edge0 < edge1; invert so the mask is 1 inside the
    // radius and 0 outside (edge0 > edge1 is unspecified in WGSL).
    let mask = 1.0 - smoothstep(threshold - 0.12, threshold, r);
    if (mask < 0.5) {
        discard;
    }

    let n = normalize(input.world_normal);
    let l = normalize(material.light_direction.xyz);
    let direct = max(dot(n, l), 0.0);

    // Vertical gradient: a touch darker toward the base for grounding/AO feel.
    let shade = 0.72 + 0.28 * input.uv.y;
    let albedo = input.tint * shade;
    let color = albedo * material.ambient.x + albedo * direct * 0.82 * material.sun_color.rgb;

    let final_color = scene_fog(color, input.world_position);

    return vec4<f32>(final_color, 1.0);
}
