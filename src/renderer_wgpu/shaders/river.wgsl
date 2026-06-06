// River shader: a translucent water surface that sits in the carved river
// channels (unlike the flat sea-level water plane). Ripples and sun glints
// travel along a per-vertex downstream flow direction so the water visibly
// streams toward the sea. Shares the hemisphere-ambient and fog helpers from
// lighting.wgsl (concatenated ahead of this file) and the sea water shader's
// shadow lookup.

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
    @location(1) flow: vec2<f32>,
    @location(2) wetness: f32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) flow: vec2<f32>,
    @location(2) wetness: f32,
};

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position =
        frame.view_proj_no_translation * vec4<f32>(input.position - frame.camera_position.xyz, 1.0);
    out.world_position = input.position;
    out.flow = input.flow;
    out.wetness = input.wetness;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Mirror of `RIVER_SURFACE_THRESHOLD` (world_core::chunk): below this the CPU
    // leaves the vertex on the bed (depth 0) and clears `has_river`. Discarding
    // at the same value keeps the rendered fringe in step with the lifted mesh,
    // so no un-lifted, bed-coplanar band survives to z-fight the terrain.
    const RIVER_WET_CUTOFF: f32 = 0.04;

    // Drop the dry fringe of the chunk grid so the surface ends in the channel
    // rather than spilling across the banks.
    if (input.wetness < RIVER_WET_CUTOFF) {
        discard;
    }

    let elapsed = frame.time.x;

    // Flow-aligned frame: `f` points downstream, `perp` across the channel. Fall
    // back to a fixed direction in the rare spot where the flow field vanishes
    // (confluences, interpolation seams) so the surface still ripples.
    let flen = length(input.flow);
    let f = select(vec2<f32>(1.0, 0.0), input.flow / flen, flen > 0.1);
    let perp = vec2<f32>(-f.y, f.x);

    // Project world position onto the flow frame. Travelling waves in `u` move
    // downstream as time advances (crests keep constant phase `u*k - t*s`, so
    // `u` grows with `t`); a gentle cross-channel term breaks up the uniformity.
    let u = dot(input.world_position.xz, f);
    let v = dot(input.world_position.xz, perp);

    let p1 = u * 0.45 - elapsed * 1.6;
    let p2 = u * 0.90 + v * 0.25 - elapsed * 2.7;
    let p3 = v * 0.60 - elapsed * 0.7;

    // Ripple height field and its slope along the flow frame, converted to a
    // perturbed world normal. Amplitudes are kept modest for a clean look but
    // big enough that the sun glint visibly rides the ripples downstream.
    let a1 = 0.11;
    let a2 = 0.07;
    let a3 = 0.04;
    let dh_du = a1 * 0.45 * cos(p1) + a2 * 0.90 * cos(p2);
    let dh_dv = a2 * 0.25 * cos(p2) + a3 * 0.60 * cos(p3);
    let slope = f * dh_du + perp * dh_dv;
    let n = normalize(vec3<f32>(-slope.x, 1.0, -slope.y));

    let l = normalize(material.light_direction.xyz);
    let diffuse = max(dot(n, l), 0.0);
    let shadow = sample_shadow(input.world_position, diffuse);
    let shade = hemisphere_ambient(n) + diffuse * shadow * 0.55;

    // Broad sun glint riding the ripples, plus a fresnel rim that sets the
    // surface off against the bed.
    let view_dir = normalize(frame.camera_position.xyz - input.world_position);
    let half_vec = normalize(l + view_dir);
    let spec = pow(max(dot(n, half_vec), 0.0), 55.0);
    let fresnel = 1.0 - max(dot(view_dir, n), 0.0);

    // Visible downstream "current": gentle light/dark streaks advected along the
    // flow. This is the clearest cue that the water is moving — it reads even
    // when the sun glints don't happen to point at the camera. Crests hold a
    // constant phase `u*k - t*s`, so they drift downstream (+flow) over time.
    let s1 = sin(u * 0.22 + v * 0.10 - elapsed * 1.2);
    let s2 = sin(u * 0.12 - v * 0.20 - elapsed * 0.65);
    let current = 1.0 + 0.11 * s1 + 0.06 * s2;

    // River water reads slightly clearer/greener than the deep sea plane.
    let river_color = vec3<f32>(0.08, 0.26, 0.38);
    let lit = (river_color * shade) * current + material.sun_color.rgb * spec * shadow * 1.0;

    // Opacity grows from the bank toward the channel centre and at glancing
    // angles, so shallow edges stay sheer and the deep middle reads as water.
    let edge = clamp((input.wetness - RIVER_WET_CUTOFF) / 0.25, 0.0, 1.0);
    let alpha = edge * mix(0.45, 0.92, fresnel * fresnel);

    let final_color = scene_fog(lit, input.world_position);
    return vec4<f32>(final_color, alpha);
}
