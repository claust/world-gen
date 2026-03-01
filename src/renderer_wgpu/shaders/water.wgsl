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

    // Diffuse lighting
    let diffuse = max(dot(n, l), 0.0);
    let shade = material.ambient.x + diffuse * 0.6;

    // Specular highlight (sun glint on water)
    let view_dir = normalize(frame.camera_position.xyz - input.world_position);
    let half_vec = normalize(l + view_dir);
    let spec = pow(max(dot(n, half_vec), 0.0), 64.0);

    // Fresnel-like effect: more opaque at glancing angles
    let fresnel = 1.0 - max(dot(view_dir, n), 0.0);
    let alpha = mix(0.45, 0.85, fresnel * fresnel);

    // Deep blue-green water color
    let water_color = vec3<f32>(0.12, 0.30, 0.45) * shade + material.sun_color.rgb * spec * 0.6;

    // Apply fog to RGB only, preserve alpha
    let dist = distance(input.world_position, frame.camera_position.xyz);
    let fog_start = material.fog_params.x;
    let fog_end = material.fog_params.y;
    let fog_factor = clamp((dist - fog_start) / (fog_end - fog_start), 0.0, 1.0);
    let final_color = mix(water_color, material.fog_color.rgb, fog_factor);

    return vec4<f32>(final_color, alpha);
}
