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

// --- Noise functions for procedural clouds ---

fn hash2d(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn noise2d(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);

    let a = hash2d(i);
    let b = hash2d(i + vec2<f32>(1.0, 0.0));
    let c = hash2d(i + vec2<f32>(0.0, 1.0));
    let d = hash2d(i + vec2<f32>(1.0, 1.0));

    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>) -> f32 {
    var val = 0.0;
    var amp = 0.5;
    var pos = p;
    for (var i = 0; i < 5; i = i + 1) {
        val = val + amp * noise2d(pos);
        pos = pos * 2.0 + vec2<f32>(1.7, 9.2);
        amp = amp * 0.5;
    }
    return val;
}

// --- End noise functions ---

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

    var final_color = sky + sun_contribution;

    // --- Procedural clouds ---
    if elevation > 0.0 {
        // Intersect ray with cloud plane at fixed altitude above camera
        let cloud_altitude = 800.0;
        let t_cloud = cloud_altitude / ray_dir.y;
        let cloud_pos = frame.camera_position.xyz + ray_dir * t_cloud;

        // Sample FBM noise at cloud position, scrolled by time
        let cloud_scale = 0.0008;
        let wind_speed = 15.0;
        let uv = cloud_pos.xz * cloud_scale + vec2<f32>(frame.time.x * wind_speed * cloud_scale, 0.0);

        let noise_val = fbm(uv);

        // Shape clouds: threshold + smoothstep for coverage control
        let coverage = 0.45;
        let density = smoothstep(coverage, coverage + 0.25, noise_val);

        // Fade clouds near horizon to avoid hard cutoff
        let horizon_fade = smoothstep(0.0, 0.15, elevation);

        let cloud_alpha = density * horizon_fade;

        // Cloud color: lit by sun, tinted by time of day
        let base_cloud = vec3<f32>(1.0, 1.0, 1.0);
        let sun_lit = mix(material.ambient.rgb, material.sun_color.rgb, 0.7);
        let cloud_color = base_cloud * sun_lit;

        final_color = mix(final_color, cloud_color, cloud_alpha);
    }

    return vec4<f32>(final_color, 1.0);
}
