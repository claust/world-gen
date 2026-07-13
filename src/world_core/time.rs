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

    /// Jump the time of day to `hour` without disturbing `total_hours`, the
    /// monotonic counter that drives plant growth and other lifecycle state —
    /// debugging the lighting shouldn't rewind the simulation.
    pub fn set_hour(&mut self, hour: f32) {
        self.hour = hour.rem_euclid(24.0);
    }

    /// Jump the monotonic simulation clock to `total_hours`, deriving the time
    /// of day from it. Used by benchmark setup to age the world so lifecycle
    /// events (stage changes, death, despawn) are actually due during the run.
    pub fn set_total_hours(&mut self, total_hours: f64) {
        self.total_hours = total_hours.max(0.0);
        self.hour = (self.total_hours % 24.0) as f32;
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
    fn set_hour_changes_time_of_day_without_touching_total_hours() {
        let mut clock = WorldClock::new(6.0, 30.0, 1.0);
        clock.set_hour(12.0);
        assert!((clock.hour() - 12.0).abs() < 1e-6);
        assert!((clock.total_hours() - 30.0).abs() < 1e-9);
    }

    #[test]
    fn set_hour_wraps_out_of_range_values() {
        let mut clock = WorldClock::new(6.0, 30.0, 1.0);
        clock.set_hour(26.0);
        assert!((clock.hour() - 2.0).abs() < 1e-6);
        clock.set_hour(-1.0);
        assert!((clock.hour() - 23.0).abs() < 1e-6);
    }

    #[test]
    fn constructor_preserves_hour_and_total_hours_independently() {
        let clock = WorldClock::new(23.5, 71.5, 1.0);
        assert!((clock.hour() - 23.5).abs() < 1e-6);
        assert!((clock.total_hours() - 71.5).abs() < 1e-9);
    }
}
