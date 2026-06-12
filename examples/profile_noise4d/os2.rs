//! Candidate B: branchless 4D simplex with prime-multiply hashing and an f32
//! core. This won the bake-off and was productionized as
//! [`world_core::noise4::Simplex4`] — see that module for the full design
//! notes (branchless corner selection, computed hash, and the empirically
//! probed kernel radius/falloff choice). This wrapper benches the production
//! type itself, so the harness always reflects what actually ships.

use super::Noise4;
use world_gen::world_core::noise4::Simplex4;

struct ProductionSimplex4(Simplex4);

impl Noise4 for ProductionSimplex4 {
    fn name(&self) -> &'static str {
        "os2: branchless 4D simplex (production noise4::Simplex4)"
    }
    fn get(&self, point: [f64; 4]) -> f64 {
        self.0.get(point)
    }
}

pub fn create(seed: u32) -> Option<Box<dyn Noise4>> {
    Some(Box::new(ProductionSimplex4(Simplex4::new(seed))))
}
