pub struct SkyPalette {
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    pub sun_color: [f32; 3],
}

struct KeyFrame {
    hour: f32,
    zenith: [f32; 3],
    horizon: [f32; 3],
    sun_color: [f32; 3],
}

const KEYFRAMES: &[KeyFrame] = &[
    KeyFrame {
        hour: 0.0,
        zenith: [0.01, 0.01, 0.05],
        horizon: [0.02, 0.02, 0.08],
        sun_color: [0.2, 0.2, 0.4],
    },
    KeyFrame {
        hour: 5.0,
        zenith: [0.05, 0.05, 0.15],
        horizon: [0.15, 0.08, 0.05],
        sun_color: [0.8, 0.4, 0.2],
    },
    KeyFrame {
        hour: 6.5,
        zenith: [0.20, 0.25, 0.50],
        horizon: [0.90, 0.50, 0.20],
        sun_color: [1.0, 0.7, 0.3],
    },
    KeyFrame {
        hour: 8.0,
        zenith: [0.35, 0.55, 0.90],
        horizon: [0.55, 0.75, 0.95],
        sun_color: [1.0, 0.95, 0.8],
    },
    KeyFrame {
        hour: 12.0,
        zenith: [0.35, 0.55, 0.90],
        horizon: [0.55, 0.75, 0.95],
        sun_color: [1.0, 1.0, 0.95],
    },
    KeyFrame {
        hour: 16.0,
        zenith: [0.35, 0.55, 0.90],
        horizon: [0.55, 0.75, 0.95],
        sun_color: [1.0, 0.95, 0.8],
    },
    KeyFrame {
        hour: 17.5,
        zenith: [0.20, 0.25, 0.50],
        horizon: [0.90, 0.50, 0.20],
        sun_color: [1.0, 0.7, 0.3],
    },
    KeyFrame {
        hour: 19.0,
        zenith: [0.05, 0.05, 0.15],
        horizon: [0.15, 0.08, 0.05],
        sun_color: [0.8, 0.4, 0.2],
    },
    KeyFrame {
        hour: 20.0,
        zenith: [0.01, 0.01, 0.05],
        horizon: [0.02, 0.02, 0.08],
        sun_color: [0.2, 0.2, 0.4],
    },
    KeyFrame {
        hour: 24.0,
        zenith: [0.01, 0.01, 0.05],
        horizon: [0.02, 0.02, 0.08],
        sun_color: [0.2, 0.2, 0.4],
    },
];

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

pub fn sky_palette(hour: f32) -> SkyPalette {
    let h = hour.rem_euclid(24.0);

    // Find the two keyframes to interpolate between
    let mut i = 0;
    while i + 1 < KEYFRAMES.len() && KEYFRAMES[i + 1].hour <= h {
        i += 1;
    }

    if i + 1 >= KEYFRAMES.len() {
        let kf = &KEYFRAMES[KEYFRAMES.len() - 1];
        return SkyPalette {
            zenith: kf.zenith,
            horizon: kf.horizon,
            sun_color: kf.sun_color,
        };
    }

    let a = &KEYFRAMES[i];
    let b = &KEYFRAMES[i + 1];
    let t = (h - a.hour) / (b.hour - a.hour);

    SkyPalette {
        zenith: lerp3(a.zenith, b.zenith, t),
        horizon: lerp3(a.horizon, b.horizon, t),
        sun_color: lerp3(a.sun_color, b.sun_color, t),
    }
}
