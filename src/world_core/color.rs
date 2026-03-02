/// Convert HSL (h in degrees, s/l in 0..1) to sRGB (0..1).
pub fn hsl_to_srgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = ((h % 360.0) + 360.0) % 360.0;
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);

    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };

    [r + m, g + m, b + m]
}

/// Convert a single sRGB channel to linear.
fn srgb_to_linear_channel(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert HSL (h in degrees, s/l in 0..1) to linear RGB.
pub fn hsl_to_linear(h: f32, s: f32, l: f32) -> [f32; 3] {
    let [r, g, b] = hsl_to_srgb(h, s, l);
    [
        srgb_to_linear_channel(r),
        srgb_to_linear_channel(g),
        srgb_to_linear_channel(b),
    ]
}
