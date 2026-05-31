use std::f64::consts::TAU;

use noise::{NoiseFn, OpenSimplex};

use crate::world_core::chunk::WORLD_SIZE_METERS;
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

    /// Sample a noise layer so the field is exactly periodic with period
    /// `L = WORLD_SIZE_METERS` on both axes, matching seamlessly across the
    /// world-wrap seam.
    ///
    /// Each world axis is mapped onto a circle and the four circle coordinates
    /// are fed to 4D OpenSimplex (a torus embedded in 4D). For requested spatial
    /// frequency `f`, the loop must close after a whole number of noise cycles,
    /// so we snap the cycle count `k = round(f * L)` to an integer (the effective
    /// frequency becomes `k / L`, a hair off `f` — negligible at these `k`). The
    /// frequency the rest of the code reasons about stays continuous.
    ///
    /// `offset_x` / `offset_z` are phase shifts expressed in the same scaled
    /// (cycles) units as the original `x * f + offset` domain offset, applied as
    /// an angular rotation so decorrelated layers stay decorrelated on the torus.
    fn torus_sample(
        noise: &OpenSimplex,
        x: f64,
        z: f64,
        frequency: f64,
        offset_x: f64,
        offset_z: f64,
    ) -> f64 {
        let l = WORLD_SIZE_METERS;
        // Whole noise cycles around the world per axis; never 0 (would flatten
        // the layer) — clamp to at least 1.
        let k = (frequency * l).round().max(1.0);
        let radius = k / TAU;
        // arc length along the loop = scaled coordinate; an offset in scaled
        // units rotates the angle by offset * TAU / k.
        let theta_x = TAU * x / l + offset_x * TAU / k;
        let theta_z = TAU * z / l + offset_z * TAU / k;
        noise.get([
            radius * theta_x.cos(),
            radius * theta_x.sin(),
            radius * theta_z.cos(),
            radius * theta_z.sin(),
        ])
    }

    pub fn sample_height(&self, x: f32, z: f32) -> f32 {
        let x = x as f64;
        let z = z as f64;
        let c = &self.config;

        let broad =
            Self::torus_sample(&self.continental, x, z, c.continental.frequency, 0.0, 0.0) as f32;
        let ridges =
            1.0 - (Self::torus_sample(&self.ridge, x, z, c.ridge.frequency, 0.0, 0.0).abs() as f32);
        let rough = Self::torus_sample(&self.detail, x, z, c.detail.frequency, 0.0, 0.0) as f32;

        broad * c.continental.amplitude + ridges * c.ridge.amplitude + rough * c.detail.amplitude
    }

    pub fn sample_moisture(&self, x: f32, z: f32) -> f32 {
        let x = x as f64;
        let z = z as f64;
        let c = &self.config;
        let base =
            Self::torus_sample(&self.moisture, x, z, c.moisture_base_frequency, 0.0, 0.0) as f32;
        let variation = Self::torus_sample(
            &self.moisture,
            x,
            z,
            c.moisture_variation_frequency,
            c.moisture_variation_offset_x,
            c.moisture_variation_offset_z,
        ) as f32;
        ((base * c.moisture_base_weight + variation * c.moisture_variation_weight) * 0.5 + 0.5)
            .clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Heightmap;
    use crate::world_core::chunk::WORLD_SIZE_METERS;
    use crate::world_core::config::HeightmapConfig;

    /// The terrain field must be exactly periodic with period `L` on both axes,
    /// so the world meets itself seamlessly across the wrap seam.
    #[test]
    fn height_and_moisture_wrap_seamlessly() {
        let hm = Heightmap::new(42, HeightmapConfig::default());
        let l = WORLD_SIZE_METERS as f32;

        for &(x, z) in &[
            (0.0, 0.0),
            (123.5, -777.0),
            (40_000.0, 5_120.0),
            (-321.0, 9_999.0),
        ] {
            let h0 = hm.sample_height(x, z);
            let m0 = hm.sample_moisture(x, z);
            // Shifting either axis by a whole world lap must reproduce the field.
            for &(dx, dz) in &[(l, 0.0), (0.0, l), (l, l), (-l, 2.0 * l)] {
                assert!(
                    (hm.sample_height(x + dx, z + dz) - h0).abs() < 1e-3,
                    "height not periodic at ({x},{z}) + ({dx},{dz})"
                );
                assert!(
                    (hm.sample_moisture(x + dx, z + dz) - m0).abs() < 1e-3,
                    "moisture not periodic at ({x},{z}) + ({dx},{dz})"
                );
            }
        }
    }
}
