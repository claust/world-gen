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

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) id: u32) -> VertexOutput {
    // Fullscreen triangle: 3 vertices covering entire screen
    let uv = vec2<f32>(f32((id << 1u) & 2u), f32(id & 2u));
    var out: VertexOutput;
    out.clip_position = vec4<f32>(uv * 2.0 - 1.0, 1.0, 1.0);
    out.ndc = out.clip_position.xy;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Reconstruct world-space ray direction from NDC
    let clip_far = vec4<f32>(input.ndc, 1.0, 1.0);
    let world_far = frame.inv_view_proj * clip_far;
    let ray_dir = normalize(world_far.xyz / world_far.w - frame.camera_position.xyz);

    // Elevation: positive above horizon, negative below
    let elevation = ray_dir.y;

    // Sky gradient: horizon at elevation=0, zenith at elevation=1
    // Use sqrt curve for a wider horizon band
    let t = pow(max(elevation, 0.0), 0.5);
    var sky = mix(material.sky_horizon.rgb, material.sky_zenith.rgb, t);

    // Below horizon: darken slightly toward nadir
    if elevation < 0.0 {
        let below = pow(min(-elevation, 1.0), 0.5);
        sky = mix(material.sky_horizon.rgb, material.sky_horizon.rgb * 0.6, below);
    }

    // Sun disc and glow
    let sun_dir = normalize(material.light_direction.xyz);
    let sun_dot = dot(ray_dir, sun_dir);

    // Sharp sun disc
    let disc = smoothstep(0.9993, 0.9998, sun_dot);

    // Soft glow around sun
    let glow = pow(max(sun_dot, 0.0), 128.0) * 0.4;
    let wide_glow = pow(max(sun_dot, 0.0), 16.0) * 0.15;

    // Sun brightness scales with how high it is above horizon
    let sun_altitude = max(sun_dir.y, 0.0);
    let sun_brightness = smoothstep(0.0, 0.15, sun_altitude);

    let sun_contribution = material.sun_color.rgb * (disc + glow + wide_glow) * sun_brightness;

    let final_color = sky + sun_contribution;

    return vec4<f32>(final_color, 1.0);
}
