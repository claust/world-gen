// Shared hemisphere ambient lighting helpers.
//
// This file has no bindings of its own. It is concatenated (via concat! +
// include_str!) ahead of any fragment shader that needs hemisphere ambient,
// and relies on that shader declaring the `material: MaterialUniform` uniform
// with `ambient`, `sky_zenith`, and `sky_horizon` fields. WGSL allows these
// module-scope references to resolve regardless of declaration order.

// Reduce a sky color to a gentle hue (max channel == 1), then blend back
// toward white so the ambient tint is subtle rather than a strong color cast.
fn hemisphere_tint(c: vec3<f32>) -> vec3<f32> {
    let m = max(max(c.r, c.g), max(c.b, 1e-3));
    let hue = c / m;
    // Scale tint strength by source brightness so very dark but saturated
    // night-sky colors stay near-neutral instead of casting a strong blue tint.
    let strength = 0.6 * clamp(m, 0.0, 1.0);
    return mix(vec3<f32>(1.0), hue, strength);
}

// Hemisphere (sky/ground) ambient: up-facing surfaces pick up the zenith sky
// color, down-facing surfaces a dimmer horizon-tinted bounce. material.ambient.x
// keeps controlling overall day/night intensity.
fn hemisphere_ambient(normal: vec3<f32>) -> vec3<f32> {
    let up = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0);
    let sky = hemisphere_tint(material.sky_zenith.rgb);
    let ground = hemisphere_tint(material.sky_horizon.rgb) * 0.6;
    return material.ambient.x * mix(ground, sky, up);
}

// Cheap 2D hash / value noise shared by the ground macro-variation below and
// the grass wind gust field. Pure functions, no bindings. The hash fracts its
// input before mixing, so it stays well-behaved at large world coordinates
// (unlike the classic sin-dot hash, which loses precision far from origin).
fn hash2(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + vec3<f32>(33.33));
    return fract((p3.x + p3.y) * p3.z);
}

fn value_noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Large-scale ground color variation: patchy ±10% brightness with a subtle
// dry-yellow hue shift in the darker patches, breaking the uniform green
// carpet at distance. Returned as an albedo multiplier. The terrain fragment
// and the grass blades apply the *same* multiplier at the same world position,
// so near-field blades and far-field ground shift hue together and the grass
// horizon stays invisible.
fn ground_macro_variation(world_xz: vec2<f32>) -> vec3<f32> {
    let v = value_noise2(world_xz * 0.013) * 0.7 + value_noise2(world_xz * 0.047) * 0.3;
    let bright = 0.90 + v * 0.20;
    let dry = 1.0 - v;
    return vec3<f32>(
        bright * (1.0 + dry * 0.10),
        bright,
        bright * (1.0 - dry * 0.12),
    );
}

// Population Lens world marker. `material.fog_params.z` carries the active lens
// radius in metres (0 when hidden). Fragments inside that horizontal camera-
// centred circle pick up a translucent green wash; the exact sampled boundary
// gets a brighter ring so the selected stats region has a clear real-world edge.
fn population_lens_overlay(color: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    let radius = material.fog_params.z;
    if (radius <= 0.0) {
        return color;
    }

    let dist = distance(world_pos.xz, frame.camera_position.xz);
    let aa = max(fwidth(dist), 0.75);
    let inside = 1.0 - smoothstep(radius - aa, radius + aa, dist);
    let ring_half_width = 5.0;
    let ring = 1.0 - smoothstep(ring_half_width, ring_half_width + aa, abs(dist - radius));

    let lens_green = vec3<f32>(0.20, 1.0, 0.42);
    let filled = mix(color, lens_green, inside * 0.18);
    let rimmed = mix(filled, lens_green, ring * 0.58);
    return rimmed + lens_green * ring * 0.18;
}

// Distance fog shared by terrain, water, and instanced/billboard fragments.
//
// Like the hemisphere helpers above this reads the `material` uniform, and it
// additionally depends on the `frame` uniform: `frame.camera_position` for the
// fog distance and `frame.time.z` for the submerge factor. Any shader that calls
// it must declare both `frame` and `material`.
//
// Above water it is the original sky-colored fog over the chunk-load radius.
// When the camera is submerged (`frame.time.z`, the per-frame submerge factor)
// it crossfades to a short-range blue-green murk: the lit color is pulled toward
// the murk even at zero distance (water absorbs light immediately, red first),
// then fogged out over a few tens of metres. The murk is scaled by the ambient
// light level so it darkens at night for free.
fn scene_fog(color: vec3<f32>, world_pos: vec3<f32>) -> vec3<f32> {
    // Constants are function-local so they are not emitted as module-global
    // constants into the vertex stage (which never calls this), which would
    // trip unused-constant warnings in the generated Metal shader.
    const UNDERWATER_TINT: vec3<f32> = vec3<f32>(0.04, 0.16, 0.22);
    const UNDERWATER_FOG_START: f32 = 1.0;
    const UNDERWATER_FOG_END: f32 = 55.0;
    const ABSORPTION: f32 = 0.4;

    let dist = distance(world_pos, frame.camera_position.xyz);

    // Above-water fog: sky-colored, long range.
    let air_factor = clamp(
        (dist - material.fog_params.x) / (material.fog_params.y - material.fog_params.x),
        0.0,
        1.0,
    );
    let air_color = mix(color, material.fog_color.rgb, air_factor);

    let submerge = clamp(frame.time.z, 0.0, 1.0);
    if (submerge <= 0.0) {
        return air_color;
    }

    // Underwater murk, dimmed toward black as ambient light drops (night/depth).
    let murk = UNDERWATER_TINT * clamp(material.ambient.x * 2.5, 0.04, 1.0);
    let absorbed = mix(color, murk, ABSORPTION);
    let water_factor = clamp(
        (dist - UNDERWATER_FOG_START) / (UNDERWATER_FOG_END - UNDERWATER_FOG_START),
        0.0,
        1.0,
    );
    let water_color = mix(absorbed, murk, water_factor);

    return mix(air_color, water_color, submerge);
}
