// Grass tuft shader: instanced wind-swaying blades, lit like terrain
// (hemisphere ambient + sun + shadow receive + fog from lighting.wgsl, which
// is concatenated ahead of this file).
//
// Instance layout matches InstanceData (shared with InstancedPass):
//   inst_color.rgb = root tint sampled from the ground albedo at the root,
//   inst_color.a   = per-tuft fade key (buffer is sorted by it, descending),
//   inst_tilt      = unused.
// Prototype-vertex `vert_color` is repurposed as packed blade data:
//   r = bend weight (0 root -> 1 tip), g = per-blade phase, b = base AO.

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
@group(2) @binding(0) var shadow_map: texture_depth_2d;
@group(2) @binding(1) var shadow_sampler: sampler_comparison;

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
    // Per-vertex (slot 0)
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) vert_color: vec3<f32>,

    // Per-instance (slot 1)
    @location(3) inst_position: vec3<f32>,
    @location(4) inst_rotation_y: f32,
    @location(5) inst_scale: vec3<f32>,
    @location(6) inst_color: vec4<f32>,
    @location(7) inst_tilt: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) albedo: vec3<f32>,
    @location(2) world_position: vec3<f32>,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    // Distance model. Must agree with GRASS_FULL_DISTANCE / GRASS_FAR_DISTANCE
    // and KEEP_MARGIN in grass_pass.rs: the CPU draws a buffer prefix sized by
    // keep_fraction * margin; the shader owns the actual per-tuft fade so a
    // changing instance count never pops.
    const GRASS_FULL: f32 = 35.0;
    const GRASS_FAR: f32 = 90.0;
    const KEEP_MARGIN: f32 = 1.12;
    const FADE_BAND: f32 = 0.10;
    // Wind model: a primary travelling wave along WIND_DIR, amplitude-
    // modulated by a slow drifting gust field, plus a small per-blade flutter.
    const WIND_DIR: vec2<f32> = vec2<f32>(0.78, 0.62);
    const WIND_STRENGTH: f32 = 0.12;

    let bend_w = input.vert_color.r;
    let blade_phase = input.vert_color.g;
    let ao = input.vert_color.b;

    // Per-tuft distance fade: tufts whose key falls below the keep threshold
    // shrink into the ground. Inside GRASS_FULL the threshold is negative
    // (KEEP_MARGIN > 1), so every tuft stands at full height.
    let d = distance(input.inst_position.xz, frame.camera_position.xz);
    let t = clamp((GRASS_FAR - d) / (GRASS_FAR - GRASS_FULL), 0.0, 1.0);
    let keep = t * t * t * KEEP_MARGIN;
    let fade = smoothstep(1.0 - keep, 1.0 - keep + FADE_BAND, input.inst_color.a);
    // Gentle height taper toward the grass horizon softens the field's edge.
    let taper = 0.55 + 0.45 * t;

    // Collapse the whole tuft toward its root as it fades so a vanishing tuft
    // becomes a point, not a flat sliver lying on the ground.
    var local = input.position * input.inst_scale * vec3<f32>(fade, fade * taper, fade);

    let c = cos(input.inst_rotation_y);
    let s = sin(input.inst_rotation_y);
    var world_pos = vec3<f32>(
        local.x * c - local.z * s,
        local.y,
        local.x * s + local.z * c,
    ) + input.inst_position;

    // Wind sway: displace along the wind with bend² weighting so roots stay
    // planted, dip the tip slightly to fake constant blade length.
    let bend = bend_w * bend_w;
    let tuft_phase = (hash2(input.inst_position.xz) + blade_phase) * 6.28318;
    let wave = sin(dot(input.inst_position.xz, WIND_DIR) * 0.35 - frame.time.x * 1.8 + tuft_phase * 0.35);
    let gust = value_noise2(input.inst_position.xz * 0.02 - WIND_DIR * (frame.time.x * 0.35));
    let flutter = sin(frame.time.x * 3.7 + tuft_phase);
    let amp = WIND_STRENGTH * (0.35 + 1.25 * gust) * input.inst_scale.y;
    let sway = (wave * 0.8 + flutter * 0.25) * amp;
    let perp = vec2<f32>(-WIND_DIR.y, WIND_DIR.x);
    let offset_xz = (WIND_DIR + perp * 0.35 * flutter) * sway * bend;
    world_pos = vec3<f32>(
        world_pos.x + offset_xz.x,
        world_pos.y - dot(offset_xz, offset_xz) * 1.2,
        world_pos.z + offset_xz.y,
    );

    // Yaw-rotate the blade normal; the fragment stage softens it toward +Y.
    let n = vec3<f32>(
        input.normal.x * c - input.normal.z * s,
        input.normal.y,
        input.normal.x * s + input.normal.z * c,
    );

    // Root tint -> lighter, warmer tip; the shared macro variation keeps the
    // blades in lockstep with the terrain albedo underneath.
    let macro_v = ground_macro_variation(input.inst_position.xz);
    let root = input.inst_color.rgb * macro_v;
    let tip = root * vec3<f32>(1.35, 1.28, 1.10);
    let albedo = mix(root, tip, bend_w) * ao;

    var out: VertexOutput;
    out.clip_position =
        frame.view_proj_no_translation * vec4<f32>(world_pos - frame.camera_position.xyz, 1.0);
    out.world_normal = n;
    out.albedo = albedo;
    out.world_position = world_pos;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Blades are two-sided and thin: lighting off the true face normal looks
    // harsh and flips across the blade, so lean it well toward +Y for soft,
    // terrain-like shading that blends with the ground.
    let n = normalize(mix(normalize(input.world_normal), vec3<f32>(0.0, 1.0, 0.0), 0.6));
    let l = normalize(material.light_direction.xyz);
    let direct = max(dot(n, l), 0.0);
    let shadow = sample_shadow(input.world_position, direct);
    let color = input.albedo * hemisphere_ambient(n)
        + input.albedo * direct * shadow * 0.82 * material.sun_color.rgb;

    let final_color = scene_fog(color, input.world_position);

    return vec4<f32>(final_color, 1.0);
}
