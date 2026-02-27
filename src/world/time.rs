use glam::Vec3;

pub struct WorldClock {
    hour: f32,
    pub day_speed: f32,
}

impl WorldClock {
    pub fn new(start_hour: f32, day_speed: f32) -> Self {
        Self {
            hour: start_hour.rem_euclid(24.0),
            day_speed,
        }
    }

    pub fn update(&mut self, dt_seconds: f32) {
        self.hour = (self.hour + dt_seconds * self.day_speed).rem_euclid(24.0);
    }

    pub fn hour(&self) -> f32 {
        self.hour
    }

    pub fn sun_direction(&self) -> Vec3 {
        let angle = (self.hour / 24.0) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let altitude = angle.sin();
        let azimuth = angle.cos();
        Vec3::new(azimuth * 0.45, altitude, 0.75).normalize()
    }

    pub fn ambient_strength(&self) -> f32 {
        let day = (self.sun_direction().y * 0.5 + 0.5).clamp(0.0, 1.0);
        0.1 + day * 0.35
    }
}
