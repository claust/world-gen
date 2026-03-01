use noise::{NoiseFn, OpenSimplex};

use crate::world_core::config::HeightmapConfig;

pub struct Heightmap {
    continental: OpenSimplex,
    ridge: OpenSimplex,
    detail: OpenSimplex,
    moisture: OpenSimplex,
    config: HeightmapConfig,
}

impl Heightmap {
    pub fn new(seed: u32, config: HeightmapConfig) -> Self {
        Self {
            continental: OpenSimplex::new(seed),
            ridge: OpenSimplex::new(seed.wrapping_add(101)),
            detail: OpenSimplex::new(seed.wrapping_add(907)),
            moisture: OpenSimplex::new(seed.wrapping_add(1701)),
            config,
        }
    }

    pub fn sample_height(&self, x: f32, z: f32) -> f32 {
        let x = x as f64;
        let z = z as f64;
        let c = &self.config;

        let broad =
            self.continental
                .get([x * c.continental.frequency, z * c.continental.frequency]) as f32;
        let ridges = 1.0
            - (self
                .ridge
                .get([x * c.ridge.frequency, z * c.ridge.frequency])
                .abs() as f32);
        let rough = self
            .detail
            .get([x * c.detail.frequency, z * c.detail.frequency]) as f32;

        broad * c.continental.amplitude + ridges * c.ridge.amplitude + rough * c.detail.amplitude
    }

    pub fn sample_moisture(&self, x: f32, z: f32) -> f32 {
        let x = x as f64;
        let z = z as f64;
        let c = &self.config;
        let base = self
            .moisture
            .get([x * c.moisture_base_frequency, z * c.moisture_base_frequency])
            as f32;
        let variation = self.moisture.get([
            x * c.moisture_variation_frequency + c.moisture_variation_offset_x,
            z * c.moisture_variation_frequency + c.moisture_variation_offset_z,
        ]) as f32;
        ((base * c.moisture_base_weight + variation * c.moisture_variation_weight) * 0.5 + 0.5)
            .clamp(0.0, 1.0)
    }
}
