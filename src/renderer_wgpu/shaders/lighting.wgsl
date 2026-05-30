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
