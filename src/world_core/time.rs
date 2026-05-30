use glam::Vec3;

pub struct WorldClock {
    hour: f32,
    total_hours: f64,
    day_speed: f32,
}

impl WorldClock {
    pub fn new(start_hour: f32, total_hours: f64, day_speed: f32) -> Self {
        Self {
            hour: start_hour.rem_euclid(24.0),
            total_hours: total_hours.max(0.0),
            day_speed,
        }
    }

    pub fn update(&mut self, dt_seconds: f32) {
        let delta_hours = dt_seconds as f64 * self.day_speed as f64;
        self.total_hours += delta_hours;
        self.hour = (self.hour + dt_seconds * self.day_speed).rem_euclid(24.0);
    }

    pub fn day_speed(&self) -> f32 {
        self.day_speed
    }

    pub fn set_day_speed(&mut self, day_speed: f32) {
        self.day_speed = day_speed;
    }

    pub fn hour(&self) -> f32 {
        self.hour
    }

    pub fn total_hours(&self) -> f64 {
        self.total_hours
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

#[cfg(test)]
mod tests {
    use super::WorldClock;

    #[test]
    fn update_advances_total_hours_monotonically() {
        let mut clock = WorldClock::new(6.0, 6.0, 2.0);
        clock.update(1.5);
        assert!((clock.total_hours() - 9.0).abs() < 1e-9);
        clock.update(2.0);
        assert!((clock.total_hours() - 13.0).abs() < 1e-9);
    }

    #[test]
    fn hour_wraps_but_total_hours_does_not() {
        let mut clock = WorldClock::new(23.5, 47.5, 1.0);
        clock.update(2.0);
        assert!((clock.hour() - 1.5).abs() < 1e-6);
        assert!((clock.total_hours() - 49.5).abs() < 1e-9);
    }

    #[test]
    fn constructor_preserves_hour_and_total_hours_independently() {
        let clock = WorldClock::new(23.5, 71.5, 1.0);
        assert!((clock.hour() - 23.5).abs() < 1e-6);
        assert!((clock.total_hours() - 71.5).abs() < 1e-9);
    }
}
