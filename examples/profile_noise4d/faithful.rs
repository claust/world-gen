//! Candidate A: faithful, specialized port of the `noise` crate's OpenSimplex
//! 4D — same lattice, same permutation-table hash, same gradient set, so the
//! output should match the baseline bit-for-bit (or to ~1e-15), while
//! stripping the crate's generic overhead (Vector4 abstractions, numcast +
//! unwrap per contribution, dyn-friendly indirection).
//!
//! Fidelity notes (every floating-point quirk of noise 0.9 is replicated):
//! - `Vector4::sum`/`dot` fold left; the crate seeds the fold with `0.0`,
//!   which is dropped here because `0.0 + v == v` for every f64 except that
//!   it can flip the sign of a zero — and no downstream consumer observes
//!   that (comparisons ignore it, the quirky floor maps ±0.0 to the same
//!   cell, and the `+0.0`-seeded value accumulator absorbs ±0.0 equally).
//! - `floor_to_isize` is NOT a true floor: it truncates toward zero and
//!   subtracts 1 for inputs `<= 0.0` (so `-1.0` maps to `-2`, `0.0` to `-1`).
//! - `t.powi(4)` is used verbatim (LLVM lowers it to two squarings).
//! - The permutation table is rebuilt with an inline copy of
//!   `rand_xorshift 0.3`'s XorShiftRng and `rand 0.8`'s Fisher–Yates shuffle
//!   (`gen_index` → 32-bit `sample_single_inclusive`), seeded exactly like
//!   `PermutationTable::new`, so the table is byte-identical to the crate's.
//!
//! Other intentional deviations (no floating-point effect): the vertex hash
//! is computed *after* the `t > 0` cull instead of before, and the per-axis
//! masked bytes / first-level permutation lookups are hoisted out of the
//! per-vertex loop (each axis offset is 0 or 1, so there are only two
//! variants per axis).

use super::Noise4;

const STRETCH_CONSTANT: f64 = -0.138_196_601_125_011; // (1/sqrt(4+1)-1)/4
const SQUISH_CONSTANT: f64 = 0.309_016_994_374_947; // (sqrt(4+1)-1)/4
const NORM_CONSTANT: f64 = 1.0 / 6.869_909_007_095_662_5;

pub fn create(seed: u32) -> Option<Box<dyn Noise4>> {
    Some(Box::new(FaithfulOpenSimplex4::new(seed)))
}

pub struct FaithfulOpenSimplex4 {
    perm: [u8; 256],
    grads: [[f64; 4]; 64],
}

// --- permutation table construction (== noise 0.9 PermutationTable::new) ---

/// Inline copy of `rand_xorshift 0.3`'s XorShiftRng core.
struct XorShiftRng {
    x: u32,
    y: u32,
    z: u32,
    w: u32,
}

impl XorShiftRng {
    /// `SeedableRng::from_seed` with the byte layout `PermutationTable::new`
    /// builds: byte 0 = 1, then the little-endian seed repeated in words 1–3.
    fn from_noise_seed(seed: u32) -> Self {
        let mut real = [0u8; 16];
        real[0] = 1;
        for i in 1..4 {
            real[i * 4] = seed as u8;
            real[(i * 4) + 1] = (seed >> 8) as u8;
            real[(i * 4) + 2] = (seed >> 16) as u8;
            real[(i * 4) + 3] = (seed >> 24) as u8;
        }
        // Word 0 contains the byte 1, so the all-zero fallback never triggers.
        Self {
            x: u32::from_le_bytes([real[0], real[1], real[2], real[3]]),
            y: u32::from_le_bytes([real[4], real[5], real[6], real[7]]),
            z: u32::from_le_bytes([real[8], real[9], real[10], real[11]]),
            w: u32::from_le_bytes([real[12], real[13], real[14], real[15]]),
        }
    }

    fn next_u32(&mut self) -> u32 {
        let x = self.x;
        let t = x ^ (x << 11);
        self.x = self.y;
        self.y = self.z;
        self.z = self.w;
        let w = self.w;
        self.w = w ^ (w >> 19) ^ (t ^ (t >> 8));
        self.w
    }

    /// `rand 0.8`'s `UniformInt::<u32>::sample_single_inclusive(0, ubound-1)`,
    /// as called by `seq::gen_index` (the slice `shuffle` helper, which uses
    /// 32-bit sampling for platform-independent output).
    fn gen_index(&mut self, ubound: u32) -> u32 {
        let range = ubound;
        let zone = (range << range.leading_zeros()).wrapping_sub(1);
        loop {
            let v = self.next_u32();
            let m = u64::from(v) * u64::from(range);
            let lo = m as u32;
            if lo <= zone {
                return (m >> 32) as u32;
            }
        }
    }
}

fn build_perm(seed: u32) -> [u8; 256] {
    let mut rng = XorShiftRng::from_noise_seed(seed);
    let mut values = [0u8; 256];
    for (i, v) in values.iter_mut().enumerate() {
        *v = i as u8;
    }
    // rand 0.8 `SliceRandom::shuffle`: Fisher–Yates from the top.
    for i in (1..values.len()).rev() {
        let j = rng.gen_index((i + 1) as u32) as usize;
        values.swap(i, j);
    }
    values
}

// --- gradient table (== noise 0.9 gradient::grad4, flattened) ---

fn build_grads() -> [[f64; 4]; 64] {
    const DIAG: f64 = 0.577_350_269_189_625_8;
    const DIAG2: f64 = 0.5;

    let mut g = [[0.0f64; 4]; 64];
    // 32 edges: one zero component, the rest ±DIAG. Index bits: the crate
    // enumerates them in blocks of 8 per zero-position (w-fastest sign order).
    let mut i = 0;
    for zero in 0..4 {
        for signs in 0..8u32 {
            let mut v = [0.0f64; 4];
            let mut bit = 0;
            for (axis, value) in v.iter_mut().enumerate() {
                if axis == zero {
                    continue;
                }
                *value = if signs & (4 >> bit) == 0 { DIAG } else { -DIAG };
                bit += 1;
            }
            // The crate's zero-position order is x, then y, z, w — but its
            // table puts the zero in slot `zero` directly, matching this.
            g[i] = v;
            i += 1;
        }
    }
    // 16 corners (±DIAG2 each), repeated twice, in the crate's listed order.
    #[rustfmt::skip]
    let corners: [[f64; 4]; 16] = [
        [ DIAG2,  DIAG2,  DIAG2,  DIAG2],
        [-DIAG2,  DIAG2,  DIAG2,  DIAG2],
        [ DIAG2, -DIAG2,  DIAG2,  DIAG2],
        [-DIAG2, -DIAG2,  DIAG2,  DIAG2],
        [ DIAG2,  DIAG2, -DIAG2,  DIAG2],
        [-DIAG2,  DIAG2, -DIAG2,  DIAG2],
        [ DIAG2,  DIAG2,  DIAG2, -DIAG2],
        [-DIAG2,  DIAG2,  DIAG2, -DIAG2],
        [ DIAG2, -DIAG2, -DIAG2,  DIAG2],
        [-DIAG2, -DIAG2, -DIAG2,  DIAG2],
        [ DIAG2, -DIAG2,  DIAG2, -DIAG2],
        [-DIAG2, -DIAG2,  DIAG2, -DIAG2],
        [ DIAG2,  DIAG2, -DIAG2, -DIAG2],
        [-DIAG2,  DIAG2, -DIAG2, -DIAG2],
        [ DIAG2, -DIAG2, -DIAG2, -DIAG2],
        [-DIAG2, -DIAG2, -DIAG2, -DIAG2],
    ];
    for (k, c) in corners.iter().enumerate() {
        g[32 + k] = *c;
        g[48 + k] = *c;
    }
    g
}

// --- the noise itself ---

/// noise 0.9's `floor_to_isize`: truncate toward zero, minus one when <= 0.
/// (Not a mathematical floor — `-1.0` maps to `-2`, `0.0` to `-1` — but the
/// baseline does exactly this, so we replicate it.)
/// Written branchlessly (subtract the comparison result) because torus
/// coordinates straddle zero, making an `if` here hard to predict.
#[inline(always)]
fn floor_to_index(v: f64) -> i64 {
    (v as i64) - (v <= 0.0) as i64
}

impl FaithfulOpenSimplex4 {
    pub fn new(seed: u32) -> Self {
        Self {
            perm: build_perm(seed),
            grads: build_grads(),
        }
    }

    #[inline(always)]
    fn eval(&self, point: [f64; 4]) -> f64 {
        let [x, y, z, w] = point;

        // Place input coordinates on the simplectic honeycomb.
        // (Vector4::sum folds left from 0.0; the seed is dropped here — it
        // can only flip the sign of a zero, which no downstream consumer
        // can observe: comparisons ignore it, the quirky floor sends ±0.0
        // to the same cell, and the value accumulator starts at +0.0.)
        let point_sum = ((x + y) + z) + w;
        let stretch_offset = point_sum * STRETCH_CONSTANT;
        let xs = x + stretch_offset;
        let ys = y + stretch_offset;
        let zs = z + stretch_offset;
        let ws = w + stretch_offset;

        // Super-cell origin in honeycomb coordinates.
        let xsb = floor_to_index(xs);
        let ysb = floor_to_index(ys);
        let zsb = floor_to_index(zs);
        let wsb = floor_to_index(ws);
        let xsf = xsb as f64;
        let ysf = ysb as f64;
        let zsf = zsb as f64;
        let wsf = wsb as f64;

        let squish_offset = (((xsf + ysf) + zsf) + wsf) * SQUISH_CONSTANT;

        // Honeycomb coordinates relative to the origin select the region.
        let region_sum = (((xs - xsf) + (ys - ysf)) + (zs - zsf)) + (ws - wsf);

        // Position relative to the unstretched origin point.
        let dx0 = x - (xsf + squish_offset);
        let dy0 = y - (ysf + squish_offset);
        let dz0 = z - (zsf + squish_offset);
        let dw0 = w - (wsf + squish_offset);

        // Hoisted hash prefix tree: each lattice vertex is the cell origin
        // plus a 0/1 offset per axis, so `PermutationTable::hash`'s left-fold
        // `perm[acc] ^ next` has only 2/4/8 distinct prefixes after the
        // x/y/z stages. The dominant region (~92% of cells) visits 10
        // vertices, so precomputing all 14 prefixes leaves one xor + one
        // lookup per vertex. All intermediate indices are < 256
        // (u8 ^ masked byte), so the unchecked accesses are in bounds by
        // construction.
        let perm = &self.perm;
        let px = [
            perm[(xsb & 0xff) as usize] as usize,
            perm[((xsb + 1) & 0xff) as usize] as usize,
        ];
        let ym = [(ysb & 0xff) as usize, ((ysb + 1) & 0xff) as usize];
        let zm = [(zsb & 0xff) as usize, ((zsb + 1) & 0xff) as usize];
        let wm = [(wsb & 0xff) as usize, ((wsb + 1) & 0xff) as usize];
        // Flat tree: index (ox << 2) | (oy << 1) | oz.
        let pxyz: [usize; 8] = unsafe {
            let mut pxy = [0usize; 4];
            for (i, &p) in px.iter().enumerate() {
                for (j, &y) in ym.iter().enumerate() {
                    pxy[(i << 1) | j] = *perm.get_unchecked(p ^ y) as usize;
                }
            }
            let mut pxyz = [0usize; 8];
            for (ij, &p) in pxy.iter().enumerate() {
                for (k, &z) in zm.iter().enumerate() {
                    pxyz[(ij << 1) | k] = *perm.get_unchecked(p ^ z) as usize;
                }
            }
            pxyz
        };

        let mut value = 0.0;

        macro_rules! contribute {
            ($ox:literal, $oy:literal, $oz:literal, $ow:literal) => {{
                // offset.sum() in the crate; all values exact here.
                const OFF_SUM: f64 = ((($ox as f64 + $oy as f64) + $oz as f64) + $ow as f64);
                const SQ: f64 = SQUISH_CONSTANT * OFF_SUM;
                let dx = (dx0 - SQ) - $ox as f64;
                let dy = (dy0 - SQ) - $oy as f64;
                let dz = (dz0 - SQ) - $oz as f64;
                let dw = (dw0 - SQ) - $ow as f64;
                let t = 2.0 - ((((dx * dx) + dy * dy) + dz * dz) + dw * dw);
                // Branchless: compute the surflet unconditionally and select
                // 0.0 for culled vertices. Adding +0.0 to the accumulator is
                // an exact identity (the accumulator is never -0.0), so this
                // matches the crate's `if t > 0.0` form bit-for-bit while
                // avoiding a hard-to-predict branch per vertex.
                let index = unsafe {
                    *perm.get_unchecked(pxyz[($ox << 2) | ($oy << 1) | $oz] ^ wm[$ow]) as usize
                };
                let g = unsafe { self.grads.get_unchecked(index % 64) };
                let dot = ((((dx * g[0]) + dy * g[1]) + dz * g[2]) + dw * g[3]);
                let surflet = if t > 0.0 { t.powi(4) * dot } else { 0.0 };
                value += surflet;
            }};
        }

        if region_sum <= 1.0 {
            // Inside the pentachoron (4-Simplex) at (0, 0, 0, 0).
            contribute!(0, 0, 0, 0);
            contribute!(1, 0, 0, 0);
            contribute!(0, 1, 0, 0);
            contribute!(0, 0, 1, 0);
            contribute!(0, 0, 0, 1);
        } else if region_sum >= 3.0 {
            // Inside the pentachoron (4-Simplex) at (1, 1, 1, 1).
            contribute!(1, 1, 1, 0);
            contribute!(1, 1, 0, 1);
            contribute!(1, 0, 1, 1);
            contribute!(0, 1, 1, 1);
            contribute!(1, 1, 1, 1);
        } else {
            // Inside one of the two dispentachora (rectified 4-Simplexes):
            // the first when region_sum <= 2.0, the second otherwise. They
            // differ only in their first four vertices and share the final
            // six in the same order, so the tail is factored out — the
            // crate's per-branch accumulation order is preserved exactly.
            // (Spatially coherent inputs make this branch predict well; a
            // branchless per-slot select was measured slower.)
            if region_sum <= 2.0 {
                contribute!(1, 0, 0, 0);
                contribute!(0, 1, 0, 0);
                contribute!(0, 0, 1, 0);
                contribute!(0, 0, 0, 1);
            } else {
                contribute!(1, 1, 1, 0);
                contribute!(1, 1, 0, 1);
                contribute!(1, 0, 1, 1);
                contribute!(0, 1, 1, 1);
            }
            contribute!(1, 1, 0, 0);
            contribute!(1, 0, 1, 0);
            contribute!(1, 0, 0, 1);
            contribute!(0, 1, 1, 0);
            contribute!(0, 1, 0, 1);
            contribute!(0, 0, 1, 1);
        }

        value * NORM_CONSTANT
    }
}

impl Noise4 for FaithfulOpenSimplex4 {
    fn name(&self) -> &'static str {
        "faithful port (candidate A)"
    }

    fn get(&self, point: [f64; 4]) -> f64 {
        self.eval(point)
    }

    fn get_row(&self, x_pairs: &[(f64, f64)], z_pair: (f64, f64), out: &mut [f64]) {
        for (o, x) in out.iter_mut().zip(x_pairs) {
            *o = self.eval([x.0, x.1, z_pair.0, z_pair.1]);
        }
    }
}
