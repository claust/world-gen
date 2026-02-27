use noise::{NoiseFn, OpenSimplex};

pub struct Heightmap {
    continental: OpenSimplex,
    ridge: OpenSimplex,
    detail: OpenSimplex,
    moisture: OpenSimplex,
}

impl Heightmap {
    pub fn new(seed: u32) -> Self {
        Self {
            continental: OpenSimplex::new(seed),
            ridge: OpenSimplex::new(seed.wrapping_add(101)),
            detail: OpenSimplex::new(seed.wrapping_add(907)),
            moisture: OpenSimplex::new(seed.wrapping_add(1701)),
        }
    }

    pub fn sample_height(&self, x: f32, z: f32) -> f32 {
        let x = x as f64;
        let z = z as f64;

        let broad = self.continental.get([x * 0.0013, z * 0.0013]) as f32;
        let ridges = 1.0 - (self.ridge.get([x * 0.0032, z * 0.0032]).abs() as f32);
        let rough = self.detail.get([x * 0.018, z * 0.018]) as f32;

        broad * 140.0 + ridges * 65.0 + rough * 10.0
    }

    pub fn sample_moisture(&self, x: f32, z: f32) -> f32 {
        let x = x as f64;
        let z = z as f64;
        let base = self.moisture.get([x * 0.0019, z * 0.0019]) as f32;
        let variation = self.moisture.get([x * 0.0095 + 31.0, z * 0.0095 - 11.0]) as f32;
        ((base * 0.75 + variation * 0.25) * 0.5 + 0.5).clamp(0.0, 1.0)
    }
}
