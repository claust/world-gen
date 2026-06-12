//! Candidate B: fast modern 4D simplex-style noise (OpenSimplex2 family) —
//! a different, cheaper lattice traversal than classic OpenSimplex. Output
//! values differ from the baseline (that's allowed; the world snapshot gets
//! regenerated), but the character should stay comparable: smooth, isotropic,
//! roughly matching the baseline's value distribution after a scale factor.

use super::Noise4;

pub fn create(_seed: u32) -> Option<Box<dyn Noise4>> {
    None // not implemented yet
}
