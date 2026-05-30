// Water shader: animated wave surface with hemisphere ambient (from
// lighting.wgsl, concatenated ahead of this file) and sun-shadow receiving.
// Water receives shadows but does not cast them.

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
@group(2) @binding(0) var shadow_map: texture_depth_2d;
@group(2) @binding(1) var shadow_sampler: sampler_comparison;

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
    @location(2) color: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var pos = input.position;

    // Gentle wave displacement
    let elapsed = frame.time.x;
    let wave1 = sin(pos.x * 0.08 + elapsed * 1.2) * 0.35;
    let wave2 = sin(pos.z * 0.06 + elapsed * 0.9) * 0.25;
    let wave3 = sin((pos.x + pos.z) * 0.12 + elapsed * 1.6) * 0.15;
    pos.y += wave1 + wave2 + wave3;

    var out: VertexOutput;
    out.clip_position = frame.view_proj * vec4<f32>(pos, 1.0);
    out.world_position = pos;

    // Approximate wave normal from derivatives
    let dx = 0.08 * cos(pos.x * 0.08 + elapsed * 1.2) * 0.35
           + 0.12 * cos((pos.x + pos.z) * 0.12 + elapsed * 1.6) * 0.15;
    let dz = 0.06 * cos(pos.z * 0.06 + elapsed * 0.9) * 0.25
           + 0.12 * cos((pos.x + pos.z) * 0.12 + elapsed * 1.6) * 0.15;
    out.world_normal = normalize(vec3<f32>(-dx, 1.0, -dz));

    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(input.world_normal);
    let l = normalize(material.light_direction.xyz);

    // Diffuse lighting with hemisphere ambient; the direct sun term is shadowed.
    let diffuse = max(dot(n, l), 0.0);
    let shadow = sample_shadow(input.world_position, diffuse);
    let shade = hemisphere_ambient(n) + diffuse * shadow * 0.6;

    // Specular highlight (sun glint on water), also occluded by shadow.
    let view_dir = normalize(frame.camera_position.xyz - input.world_position);
    let half_vec = normalize(l + view_dir);
    let spec = pow(max(dot(n, half_vec), 0.0), 64.0);

    // Fresnel-like effect: more opaque at glancing angles
    let fresnel = 1.0 - max(dot(view_dir, n), 0.0);
    let alpha = mix(0.45, 0.85, fresnel * fresnel);

    // Deep blue-green water color
    let water_color = vec3<f32>(0.12, 0.30, 0.45) * shade
        + material.sun_color.rgb * spec * shadow * 0.6;

    // Apply fog to RGB only, preserve alpha
    let dist = distance(input.world_position, frame.camera_position.xyz);
    let fog_start = material.fog_params.x;
    let fog_end = material.fog_params.y;
    let fog_factor = clamp((dist - fog_start) / (fog_end - fog_start), 0.0, 1.0);
    let final_color = mix(water_color, material.fog_color.rgb, fog_factor);

    return vec4<f32>(final_color, alpha);
}
