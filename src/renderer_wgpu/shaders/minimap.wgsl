struct MinimapUniform {
    screen_size: vec2<f32>,
    _pad: vec2<f32>,
    map_rect: vec4<f32>,
};

@group(0) @binding(0) var<uniform> hud: MinimapUniform;

@group(1) @binding(0) var map_texture: texture_2d<f32>;
@group(1) @binding(1) var map_sampler: sampler;

struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // [mode, param]: mode 1 selects the FOV-sector fragment path, where the param
    // is the sector half-angle and `uv` is the local position (forward = +Y,
    // radius normalized to 1). Mode 2 selects a circle/annulus where the param is
    // the normalized inner radius. Otherwise an ordinary solid/textured vertex.
    @location(3) sdf: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) sdf: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let ndc_x = (in.position.x / hud.screen_size.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (in.position.y / hud.screen_size.y) * 2.0;
    out.clip_position = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.sdf = in.sdf;
    return out;
}

// Signed distance to a circular sector ("pie") of radius 1, bisector along +Y,
// half-aperture encoded as c = (sin(half), cos(half)). Negative inside.
// After Inigo Quilez's sdPie.
fn sd_sector(p_in: vec2<f32>, c: vec2<f32>) -> f32 {
    let p = vec2<f32>(abs(p_in.x), p_in.y);
    let l = length(p) - 1.0;
    let m = length(p - c * clamp(dot(p, c), 0.0, 1.0));
    return max(l, m * sign(c.y * p.x - c.x * p.y));
}

fn minimap_rect_coverage(p: vec2<f32>) -> f32 {
    let rect_min = hud.map_rect.xy;
    let rect_max = hud.map_rect.xy + hud.map_rect.zw;
    if (p.x < rect_min.x || p.y < rect_min.y || p.x > rect_max.x || p.y > rect_max.y) {
        return 0.0;
    }
    return 1.0;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample unconditionally (uniform control flow required by WebGPU)
    let tex = textureSample(map_texture, map_sampler, max(in.uv, vec2(0.0)));

    // Sector coverage — computed unconditionally so the derivatives used for
    // anti-aliasing stay in uniform control flow.
    let half = in.sdf.y;
    let d = sd_sector(in.uv, vec2<f32>(sin(half), cos(half)));
    let w = max(fwidth(d) * 0.5, 1e-5);
    let coverage = 1.0 - smoothstep(-w, w, d);
    // Distance fog: opaque near the camera (apex), dissolving toward the far arc.
    let fog = 1.0 - smoothstep(0.0, 1.0, length(in.uv));
    let rect_clip = minimap_rect_coverage(in.clip_position.xy);

    // FOV sector: alpha-masked by the anti-aliased coverage and fogged by range.
    if (in.sdf.x > 0.5 && in.sdf.x < 1.5) {
        return vec4<f32>(in.color.rgb, in.color.a * coverage * fog * rect_clip);
    }
    // Circle or annulus: used for the population-lens radius marker.
    if (in.sdf.x > 1.5) {
        let dist = length(in.uv);
        let circle_w = max(fwidth(dist) * 0.5, 1e-5);
        let outer = 1.0 - smoothstep(1.0 - circle_w, 1.0 + circle_w, dist);
        let inner = smoothstep(in.sdf.y - circle_w, in.sdf.y + circle_w, dist);
        return vec4<f32>(in.color.rgb, in.color.a * outer * inner * rect_clip);
    }
    // Solid-color vertices use negative UV as sentinel
    if (in.uv.x < 0.0) {
        return in.color;
    }
    return vec4<f32>(tex.rgb, tex.a * in.color.a);
}
