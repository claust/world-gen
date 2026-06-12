//! Candidate C: SIMD/batched 4D noise. Exploits the call pattern: terrain
//! evaluates whole rows where the z circle-pair is fixed and the x pairs vary,
//! so multiple lattice points (or multiple contributions of one point) can be
//! evaluated per instruction. Must stay portable: arm64 (local), x86_64 (CI),
//! and wasm32 — so use `wide` / autovectorizable scalar code, not platform
//! intrinsics. Override `get_row` for the batched path.

use super::Noise4;

pub fn create(_seed: u32) -> Option<Box<dyn Noise4>> {
    None // not implemented yet
}
