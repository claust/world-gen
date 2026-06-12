//! Candidate B: fast modern 4D simplex-class noise. Output values differ from
//! the baseline (that's allowed; the world snapshot gets regenerated), but the
//! character is calibrated to match: smooth, isotropic, std ≈ 0.30 and
//! feature size (roughness ≈ 0.067) like the baseline detail octave.
//!
//! Design: classic 4D simplex traversal (skew → rank the in-cell offsets →
//! 5 corner contributions) but fully **branchless** — corner selection is
//! integer mask arithmetic, the radial falloff is `max(0, r² − d²)²`, and
//! vertex hashing is OpenSimplex2-style (per-axis 64-bit primes + one
//! multiply/xor, no permutation-table dependency chains). All five corners
//! form independent dependency chains, so the CPU can overlap them; there are
//! no data-dependent branches at all. Inner math runs in f32; inputs are
//! reduced to cell-relative offsets right after the frequency prescale (f32
//! quantization steps are ~1e-4 of the amplitude — far below visibility).
//!
//! Kernel-radius/exponent choice, found empirically with a finite-difference
//! continuity probe and rendered side-by-side against the baseline:
//! - r² = 0.6 (Gustavson's classic choice) is NOT C0 in 4D: rank-order swap
//!   boundaries pass within kernel range of the swapped vertices, leaving
//!   visible-in-principle creases (~0.4% of std per step). r² = 0.5 probes
//!   clean at every scale (bounded difference quotients down to f32 ulp).
//! - At r² = 0.5 a quartic kernel degrades into isolated polka-dot blobs:
//!   4D simplex cells are thin, so narrow kernels barely overlap (the same
//!   under-coverage that motivates OpenSimplex2's 5-lattice-copy scheme —
//!   which probes ~3× slower here due to its longer dependency chain).
//!   The **quadratic** kernel keeps support inside the safe radius (exactly
//!   C1 at the boundary) while spreading mass wide enough to reconnect the
//!   field; the rendered character closely matches baseline OpenSimplex.

use super::Noise4;

// Per-axis primes + hash multiplier (OpenSimplex2 constants; any good odd
// 64-bit constants work, these are well-tested for lattice decorrelation).
const PRIME_X: i64 = 0x5205_402B_9270_C86F_u64 as i64;
const PRIME_Y: i64 = 0x598C_D327_0038_17B5_u64 as i64;
const PRIME_Z: i64 = 0x5BCC_226E_9FA0_BACB_u64 as i64;
const PRIME_W: i64 = 0x56CC_5227_E58F_554B_u64 as i64;
const HASH_MULTIPLIER: i64 = 0x53A3_F72D_EEC5_46F5_u64 as i64;

const F4: f64 = 0.309_016_994_374_947_45; // (sqrt(5)-1)/4
const G4: f32 = 0.138_196_60; // (5-sqrt(5))/20
const RSQUARED: f32 = 0.5;

const N_GRADS_EXP: u32 = 8;
const N_GRADS: usize = 1 << N_GRADS_EXP;

/// Frequency prescale so feature size matches the baseline's terrain
/// character (classic OpenSimplex has larger features per lattice unit).
const INPUT_SCALE: f64 = 0.638;
/// Output scale applied to unit gradients, calibrated so the harness'
/// detail-octave workload lands at std ≈ 0.30 like the baseline.
const RESCALE: f32 = 15.2;

pub struct Simplex4 {
    seed: i64,
    grads: Box<[[f32; 4]; N_GRADS]>,
}

/// splitmix64 — expands the u32 seed into well-mixed 64-bit values.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Simplex4 {
    pub fn new(seed: u32) -> Self {
        let mut state = seed as u64;
        let lattice_seed = splitmix64(&mut state) as i64;

        // Classic 4D simplex gradient set: the 32 permutations of
        // (0, ±1, ±1, ±1), normalized to unit length, scaled to calibrated
        // output amplitude. Filled into a power-of-two table (deterministic
        // shuffle so index bits don't correlate with direction in a fixed
        // pattern); every vector appears equally often.
        const INV_SQRT3: f32 = 0.577_350_26;
        let mut base = [[0.0f32; 4]; 32];
        let mut n = 0;
        for zero_axis in 0..4 {
            for signs in 0..8u32 {
                let mut v = [0.0f32; 4];
                let mut bit = 0;
                for (axis, c) in v.iter_mut().enumerate() {
                    if axis == zero_axis {
                        continue;
                    }
                    let sign = if signs >> bit & 1 == 1 { -1.0 } else { 1.0 };
                    *c = sign * INV_SQRT3 * RESCALE;
                    bit += 1;
                }
                base[n] = v;
                n += 1;
            }
        }
        let mut order: [usize; N_GRADS] = [0; N_GRADS];
        for (i, o) in order.iter_mut().enumerate() {
            *o = i % 32;
        }
        for i in (1..N_GRADS).rev() {
            let j = (splitmix64(&mut state) % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let mut grads = Box::new([[0.0f32; 4]; N_GRADS]);
        for (i, &o) in order.iter().enumerate() {
            grads[i] = base[o];
        }
        Self {
            seed: lattice_seed,
            grads,
        }
    }

    /// One corner: quadratic falloff times hashed gradient, fully branchless.
    #[inline(always)]
    fn contrib(
        &self,
        xp: i64,
        yp: i64,
        zp: i64,
        wp: i64,
        dx: f32,
        dy: f32,
        dz: f32,
        dw: f32,
    ) -> f32 {
        let t = RSQUARED - ((dx * dx + dy * dy) + (dz * dz + dw * dw));
        let t = t.max(0.0);
        let mut hash = self.seed ^ (xp ^ yp) ^ (zp ^ wp);
        hash = hash.wrapping_mul(HASH_MULTIPLIER);
        hash ^= hash >> (64 - N_GRADS_EXP);
        let g = &self.grads[hash as usize & (N_GRADS - 1)];
        (t * t) * ((g[0] * dx + g[1] * dy) + (g[2] * dz + g[3] * dw))
    }

    #[inline(always)]
    fn eval(&self, x: f32, y: f32, z: f32, w: f32) -> f32 {
        // Skew onto the integer grid and split into cell + offsets.
        let s = (x + y + z + w) * (F4 as f32);
        let ib = fast_floor(x + s);
        let jb = fast_floor(y + s);
        let kb = fast_floor(z + s);
        let lb = fast_floor(w + s);
        let t = (ib + jb + kb + lb) as f32 * G4;
        let x0 = x - ib as f32 + t;
        let y0 = y - jb as f32 + t;
        let z0 = z - kb as f32 + t;
        let w0 = w - lb as f32 + t;

        // Rank the offsets (6 comparisons) to order the simplex traversal.
        let xy = (x0 > y0) as i32;
        let xz = (x0 > z0) as i32;
        let xw = (x0 > w0) as i32;
        let yz = (y0 > z0) as i32;
        let yw = (y0 > w0) as i32;
        let zw = (z0 > w0) as i32;
        let rx = xy + xz + xw;
        let ry = (1 - xy) + yz + yw;
        let rz = (2 - xz - yz) + zw;
        let rw = 3 - xw - yw - zw;

        // Per-axis step masks for the three intermediate corners.
        let i1 = (rx >= 3) as i32;
        let j1 = (ry >= 3) as i32;
        let k1 = (rz >= 3) as i32;
        let l1 = (rw >= 3) as i32;
        let i2 = (rx >= 2) as i32;
        let j2 = (ry >= 2) as i32;
        let k2 = (rz >= 2) as i32;
        let l2 = (rw >= 2) as i32;
        let i3 = (rx >= 1) as i32;
        let j3 = (ry >= 1) as i32;
        let k3 = (rz >= 1) as i32;
        let l3 = (rw >= 1) as i32;

        // Pre-multiplied lattice coordinates; steps add the prime under a
        // mask (branchless select).
        let ip = (ib as i64).wrapping_mul(PRIME_X);
        let jp = (jb as i64).wrapping_mul(PRIME_Y);
        let kp = (kb as i64).wrapping_mul(PRIME_Z);
        let lp = (lb as i64).wrapping_mul(PRIME_W);
        let step = |p: i64, prime: i64, m: i32| p.wrapping_add(prime & -(m as i64));

        let v0 = self.contrib(ip, jp, kp, lp, x0, y0, z0, w0);
        let v1 = self.contrib(
            step(ip, PRIME_X, i1),
            step(jp, PRIME_Y, j1),
            step(kp, PRIME_Z, k1),
            step(lp, PRIME_W, l1),
            x0 - i1 as f32 + G4,
            y0 - j1 as f32 + G4,
            z0 - k1 as f32 + G4,
            w0 - l1 as f32 + G4,
        );
        let v2 = self.contrib(
            step(ip, PRIME_X, i2),
            step(jp, PRIME_Y, j2),
            step(kp, PRIME_Z, k2),
            step(lp, PRIME_W, l2),
            x0 - i2 as f32 + 2.0 * G4,
            y0 - j2 as f32 + 2.0 * G4,
            z0 - k2 as f32 + 2.0 * G4,
            w0 - l2 as f32 + 2.0 * G4,
        );
        let v3 = self.contrib(
            step(ip, PRIME_X, i3),
            step(jp, PRIME_Y, j3),
            step(kp, PRIME_Z, k3),
            step(lp, PRIME_W, l3),
            x0 - i3 as f32 + 3.0 * G4,
            y0 - j3 as f32 + 3.0 * G4,
            z0 - k3 as f32 + 3.0 * G4,
            w0 - l3 as f32 + 3.0 * G4,
        );
        let v4 = self.contrib(
            ip.wrapping_add(PRIME_X),
            jp.wrapping_add(PRIME_Y),
            kp.wrapping_add(PRIME_Z),
            lp.wrapping_add(PRIME_W),
            x0 - 1.0 + 4.0 * G4,
            y0 - 1.0 + 4.0 * G4,
            z0 - 1.0 + 4.0 * G4,
            w0 - 1.0 + 4.0 * G4,
        );
        (v0 + v1) + (v2 + v3) + v4
    }
}

#[inline(always)]
fn fast_floor(x: f32) -> i32 {
    let xi = x as i32;
    xi - (x < xi as f32) as i32
}

impl Noise4 for Simplex4 {
    fn name(&self) -> &'static str {
        "os2: branchless 4D simplex, prime-hash, f32 core"
    }

    #[inline]
    fn get(&self, point: [f64; 4]) -> f64 {
        self.eval(
            (point[0] * INPUT_SCALE) as f32,
            (point[1] * INPUT_SCALE) as f32,
            (point[2] * INPUT_SCALE) as f32,
            (point[3] * INPUT_SCALE) as f32,
        ) as f64
    }

    fn get_row(&self, x_pairs: &[(f64, f64)], z_pair: (f64, f64), out: &mut [f64]) {
        let z0 = (z_pair.0 * INPUT_SCALE) as f32;
        let z1 = (z_pair.1 * INPUT_SCALE) as f32;
        for (o, x) in out.iter_mut().zip(x_pairs) {
            *o = self.eval(
                (x.0 * INPUT_SCALE) as f32,
                (x.1 * INPUT_SCALE) as f32,
                z0,
                z1,
            ) as f64;
        }
    }
}

pub fn create(seed: u32) -> Option<Box<dyn Noise4>> {
    Some(Box::new(Simplex4::new(seed)))
}
