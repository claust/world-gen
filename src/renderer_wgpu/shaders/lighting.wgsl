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

// Distance fog shared by terrain, water, and instanced/billboard fragments.
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
