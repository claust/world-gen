//! Candidate A: faithful, specialized port of the `noise` crate's OpenSimplex
//! 4D — same lattice, same permutation-table hash, same gradient set, so the
//! output should match the baseline bit-for-bit (or to ~1e-15), while
//! stripping the crate's generic overhead (Vector4 abstractions, numcast +
//! unwrap per contribution, dyn-friendly indirection).

use super::Noise4;

pub fn create(_seed: u32) -> Option<Box<dyn Noise4>> {
    None // not implemented yet
}
