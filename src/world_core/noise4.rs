//! Custom 4D noise core for terrain generation.
//!
//! Replaces the `noise` crate's classic OpenSimplex (~31 ns/eval) with a
//! branchless 4D simplex evaluator (~14 ns/eval, ~2.2×) measured on the real
//! detail-octave call pattern (`examples/profile_noise4d`). The output is a
//! *different* noise field than OpenSimplex — swapping it bumps
//! `BaseGenerationInputs.version` so old snapshots auto-reject — but its
//! character is calibrated to match: same feature size per coordinate unit
//! (`INPUT_SCALE`) and same value spread (`RESCALE`, std ≈ 0.30), so the
//! existing octave frequencies and amplitudes keep their meaning.
//!
//! Why it is faster than the crate:
//! - always exactly 5 corner contributions (classic OpenSimplex: 5–10), with
//!   corner selection done by integer mask arithmetic — no data-dependent
//!   branches, and the five contributions are independent dependency chains
//!   the CPU overlaps;
//! - lattice-vertex hashing is one multiply/xor over per-axis primes instead
//!   of the crate's chain of four dependent permutation-table lookups;
//! - the inner math runs in f32 (quantization is ~1e-4 of the amplitude,
//!   far below anything terrain-visible).
//!
//! Kernel shape (radius/falloff) was chosen empirically with a
//! finite-difference continuity probe, not taken from the textbook:
//! - the classic kernel radius r² = 0.6 is **not continuous** in 4D — rank-
//!   order swap boundaries pass within kernel range of swapped vertices,
//!   leaving creases; r² = 0.5 probes clean down to f32 ulp;
//! - at r² = 0.5 the usual quartic falloff renders as disconnected polka-dot
//!   blobs (4D simplex cells are thin, narrow kernels barely overlap), so the
//!   falloff is the wider **quadratic** `max(0, r² − d²)²` — exactly C1 at
//!   the support boundary — which restores OpenSimplex-like connected
//!   character.
//!
//! Pure scalar, stable, dependency-free Rust: identical output on arm64
//! macOS, x86_64, and wasm32.

// Per-axis primes + hash multiplier (OpenSimplex2's well-tested constants for
// lattice decorrelation; any good odd 64-bit constants would do).
const PRIME_X: i64 = 0x5205_402B_9270_C86F_u64 as i64;
const PRIME_Y: i64 = 0x598C_D327_0038_17B5_u64 as i64;
const PRIME_Z: i64 = 0x5BCC_226E_9FA0_BACB_u64 as i64;
const PRIME_W: i64 = 0x56CC_5227_E58F_554B_u64 as i64;
const HASH_MULTIPLIER: i64 = 0x53A3_F72D_EEC5_46F5_u64 as i64;

const F4: f64 = 0.309_016_994_374_947_45; // (sqrt(5)-1)/4
const G4: f32 = 0.138_196_6; // (5-sqrt(5))/20
const RSQUARED: f32 = 0.5;

const N_GRADS_EXP: u32 = 8;
const N_GRADS: usize = 1 << N_GRADS_EXP;

/// Frequency prescale so feature size per input unit matches the classic
/// OpenSimplex this replaced (simplex features are larger per lattice unit).
const INPUT_SCALE: f64 = 0.638;
/// Gradient scale calibrated on the detail-octave workload so the output
/// lands at std ≈ 0.30, matching the replaced OpenSimplex — existing octave
/// amplitudes then carry over unchanged.
const RESCALE: f32 = 15.2;

/// Branchless 4D simplex noise, calibrated as a drop-in replacement for the
/// `noise` crate's `OpenSimplex::get`. Deterministic from the seed on every
/// platform.
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
        // (0, ±1, ±1, ±1), normalized, scaled to the calibrated amplitude.
        // Filled into a power-of-two table with a deterministic shuffle so
        // index bits don't correlate with direction in a fixed pattern;
        // every vector appears equally often.
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

    /// Drop-in for `noise::NoiseFn::get`: same `[f64; 4]` point shape, output
    /// distribution calibrated to the replaced OpenSimplex (std ≈ 0.30).
    #[inline]
    pub fn get(&self, point: [f64; 4]) -> f64 {
        self.eval(
            (point[0] * INPUT_SCALE) as f32,
            (point[1] * INPUT_SCALE) as f32,
            (point[2] * INPUT_SCALE) as f32,
            (point[3] * INPUT_SCALE) as f32,
        ) as f64
    }

    /// One corner: quadratic falloff times hashed gradient, fully branchless.
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
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

#[cfg(test)]
mod tests {
    use super::Simplex4;

    /// Same seed must give the same field across constructions (the table
    /// shuffle and lattice seed are pure functions of the seed).
    #[test]
    fn deterministic_from_seed() {
        let a = Simplex4::new(949);
        let b = Simplex4::new(949);
        let c = Simplex4::new(950);
        let mut differs = false;
        for i in 0..1000 {
            let p = [
                (i % 37) as f64 * 0.91 - 17.0,
                (i % 53) as f64 * 0.73 - 19.0,
                (i % 71) as f64 * 0.57 - 21.0,
                (i % 89) as f64 * 0.41 - 23.0,
            ];
            assert_eq!(a.get(p).to_bits(), b.get(p).to_bits());
            differs |= a.get(p) != c.get(p);
        }
        assert!(differs, "different seeds must produce different fields");
    }

    /// The calibration contract the terrain octaves rely on: output spread
    /// close to the replaced OpenSimplex (std ≈ 0.30) and values bounded
    /// well inside [-1.5, 1.5] so amplitude budgets hold.
    #[test]
    fn calibrated_distribution() {
        let n = Simplex4::new(42);
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        let mut count = 0u32;
        for i in 0..200 {
            for j in 0..200 {
                let v = n.get([
                    i as f64 * 0.83 - 80.0,
                    i as f64 * 0.31 + 3.0,
                    j as f64 * 0.79 - 75.0,
                    j as f64 * 0.37 + 5.0,
                ]);
                assert!(v.abs() < 1.5, "value {v} out of expected range");
                sum += v;
                sum_sq += v * v;
                count += 1;
            }
        }
        let mean = sum / count as f64;
        let std = (sum_sq / count as f64 - mean * mean).sqrt();
        assert!(mean.abs() < 0.05, "mean {mean} drifted from 0");
        assert!(
            (0.2..=0.4).contains(&std),
            "std {std} drifted from the ≈0.30 calibration"
        );
    }
}
