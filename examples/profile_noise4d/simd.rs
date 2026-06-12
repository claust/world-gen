//! Candidate C: SIMD/batched 4D noise. Exploits the call pattern: terrain
//! evaluates whole rows where the z circle-pair is fixed and the x pairs vary,
//! so four grid points are evaluated per instruction stream.
//!
//! Approach: branchless 4D *simplex* noise (Gustavson skew, 5 corners). The
//! simplex-corner ranking, corner offsets, gradient selection and surflet
//! falloff are all computed with compare-masks — no per-lane branches — and
//! gradient hashing is computed (multiplicative lattice hash + avalanche)
//! instead of a permutation-table lookup, so lanes never gather from memory.
//!
//! The hot path is hand-written NEON (`core::arch::aarch64`), cfg-gated, with
//! a scalar fallback that mirrors the vector code operation-for-operation
//! (same IEEE ops in the same association order), so every platform produces
//! the bit-identical field. No fused multiply-adds anywhere. (The `wide`
//! crate was tried first: its NEON types defeat LLVM's constant folding —
//! every splat constant became a per-block `memset_pattern16` libcall — which
//! made it slower than scalar.)
//!
//! Output is a different noise than the crate's OpenSimplex; `FREQ`/`AMP`
//! below are calibrated so std and roughness match the baseline's terrain
//! character (std ≈ 0.30, roughness ≈ 0.067).

use super::Noise4;

// 4D simplex skew constants: F4 = (sqrt(5)-1)/4, G4 = (5-sqrt(5))/20.
const F4: f32 = 0.309_017;
const G4: f32 = 0.138_196_6;
/// Surflet kernel radius²: 0.5 keeps every surflet inside the region where
/// its corner is part of the evaluated corner set, so the field has no seams
/// and the (r²-d²)⁴ kernel fades to zero with zero derivative (≥ C1).
const R2: f32 = 0.5;

// Lattice-hash constants (odd, high-entropy) and an avalanche multiplier.
const P1: u32 = 0x8DA6_B343;
const P2: u32 = 0xD816_3841;
const P3: u32 = 0xCB1A_B31F;
const P4: u32 = 0x9E37_79B1;
const MIX: u32 = 0x85EB_CA6B;
const P1234: u32 = P1.wrapping_add(P2).wrapping_add(P3).wrapping_add(P4);

/// Frequency rescale so the lattice density (roughness proxy) matches the
/// baseline OpenSimplex character; amplitude rescale targets std ≈ 0.30.
/// Both calibrated against the harness `quality` stats.
const FREQ: f64 = 0.5896;
const AMP: f32 = 72.9;

pub fn create(seed: u32) -> Option<Box<dyn Noise4>> {
    Some(Box::new(SimdSimplex4 { seed }))
}

/// NEON path: 4 points per call, everything in vector registers.
#[cfg(all(target_arch = "aarch64", target_feature = "neon"))]
mod lanes {
    use core::arch::aarch64::*;

    /// One surflet: avalanche the lattice hash, pick a gradient from the
    /// "one coordinate zeroed, signs on the rest" 32-direction family
    /// (computed from hash bits, no table), apply the (r²-d²)⁴ falloff.
    #[inline(always)]
    unsafe fn corner(
        h: uint32x4_t,
        dx: float32x4_t,
        dy: float32x4_t,
        dz: float32x4_t,
        dw: float32x4_t,
    ) -> float32x4_t {
        let h = veorq_u32(h, vshrq_n_u32(h, 15));
        let h = vmulq_u32(h, vdupq_n_u32(super::MIX));
        let h = veorq_u32(h, vshrq_n_u32(h, 13));
        // Sign bits from hash bits 25..28 (shifted into the f32 sign
        // position), zeroed-coordinate index from bits 29..30.
        let sign = vdupq_n_u32(0x8000_0000);
        let gx = veorq_u32(
            vreinterpretq_u32_f32(dx),
            vandq_u32(vshlq_n_u32(h, 6), sign),
        );
        let gy = veorq_u32(
            vreinterpretq_u32_f32(dy),
            vandq_u32(vshlq_n_u32(h, 5), sign),
        );
        let gz = veorq_u32(
            vreinterpretq_u32_f32(dz),
            vandq_u32(vshlq_n_u32(h, 4), sign),
        );
        let gw = veorq_u32(
            vreinterpretq_u32_f32(dw),
            vandq_u32(vshlq_n_u32(h, 3), sign),
        );
        let zi = vandq_u32(vshrq_n_u32(h, 29), vdupq_n_u32(3));
        let t0 = vbicq_u32(gx, vceqq_u32(zi, vdupq_n_u32(0)));
        let t1 = vbicq_u32(gy, vceqq_u32(zi, vdupq_n_u32(1)));
        let t2 = vbicq_u32(gz, vceqq_u32(zi, vdupq_n_u32(2)));
        let t3 = vbicq_u32(gw, vceqq_u32(zi, vdupq_n_u32(3)));
        let gd = vaddq_f32(
            vaddq_f32(vreinterpretq_f32_u32(t0), vreinterpretq_f32_u32(t1)),
            vaddq_f32(vreinterpretq_f32_u32(t2), vreinterpretq_f32_u32(t3)),
        );
        let tt = vsubq_f32(
            vsubq_f32(
                vsubq_f32(
                    vsubq_f32(vdupq_n_f32(super::R2), vmulq_f32(dx, dx)),
                    vmulq_f32(dy, dy),
                ),
                vmulq_f32(dz, dz),
            ),
            vmulq_f32(dw, dw),
        );
        let tt = vmaxq_f32(tt, vdupq_n_f32(0.0));
        let tsq = vmulq_f32(tt, tt);
        vmulq_f32(vmulq_f32(tsq, tsq), gd)
    }

    /// Evaluate 4 points sharing scalar (z, w); inputs already
    /// frequency-scaled.
    #[inline(always)]
    pub fn eval4(seed: u32, xs: &[f32; 4], ys: &[f32; 4], z: f32, w: f32) -> [f32; 4] {
        unsafe {
            let x = vld1q_f32(xs.as_ptr());
            let y = vld1q_f32(ys.as_ptr());
            let zf = vdupq_n_f32(z);
            let wf = vdupq_n_f32(w);

            // Skew to lattice space, find the base cell + in-cell position.
            let s = vmulq_f32(
                vaddq_f32(vaddq_f32(x, y), vdupq_n_f32(z + w)),
                vdupq_n_f32(super::F4),
            );
            let fi = vrndmq_f32(vaddq_f32(x, s));
            let fj = vrndmq_f32(vaddq_f32(y, s));
            let fk = vrndmq_f32(vaddq_f32(zf, s));
            let fl = vrndmq_f32(vaddq_f32(wf, s));
            let ii = vreinterpretq_u32_s32(vcvtq_s32_f32(fi));
            let jj = vreinterpretq_u32_s32(vcvtq_s32_f32(fj));
            let kk = vreinterpretq_u32_s32(vcvtq_s32_f32(fk));
            let ll = vreinterpretq_u32_s32(vcvtq_s32_f32(fl));
            let t = vmulq_f32(
                vaddq_f32(vaddq_f32(vaddq_f32(fi, fj), fk), fl),
                vdupq_n_f32(super::G4),
            );
            let x0 = vaddq_f32(vsubq_f32(x, fi), t);
            let y0 = vaddq_f32(vsubq_f32(y, fj), t);
            let z0 = vaddq_f32(vsubq_f32(zf, fk), t);
            let w0 = vaddq_f32(vsubq_f32(wf, fl), t);

            // Branchless coordinate ranking decides which simplex of the
            // cell we're in: rank[c] = how many other coordinates c beats
            // (ties go to the later coordinate, as in the classic code).
            let one = vdupq_n_u32(1);
            let cxy = vandq_u32(vcgtq_f32(x0, y0), one);
            let cxz = vandq_u32(vcgtq_f32(x0, z0), one);
            let cxw = vandq_u32(vcgtq_f32(x0, w0), one);
            let cyz = vandq_u32(vcgtq_f32(y0, z0), one);
            let cyw = vandq_u32(vcgtq_f32(y0, w0), one);
            let czw = vandq_u32(vcgtq_f32(z0, w0), one);
            let rx = vaddq_u32(vaddq_u32(cxy, cxz), cxw);
            let ry = vaddq_u32(vaddq_u32(veorq_u32(cxy, one), cyz), cyw);
            let rz = vaddq_u32(vaddq_u32(veorq_u32(cxz, one), veorq_u32(cyz, one)), czw);
            let rw = vaddq_u32(
                vaddq_u32(veorq_u32(cxw, one), veorq_u32(cyw, one)),
                veorq_u32(czw, one),
            );

            // Additive lattice hash: each traversal corner just adds its
            // selected per-axis constants to the base, so the only integer
            // multiplies are the base products and the per-corner avalanche.
            let hb = vaddq_u32(
                vaddq_u32(
                    vaddq_u32(
                        vmulq_u32(ii, vdupq_n_u32(super::P1)),
                        vmulq_u32(jj, vdupq_n_u32(super::P2)),
                    ),
                    vaddq_u32(
                        vmulq_u32(kk, vdupq_n_u32(super::P3)),
                        vmulq_u32(ll, vdupq_n_u32(super::P4)),
                    ),
                ),
                vdupq_n_u32(seed),
            );

            let mut sum = corner(hb, x0, y0, z0, w0);
            let onef = vdupq_n_u32(0x3F80_0000); // 1.0f32 bit pattern
            for n in 1..=3u32 {
                // rank >= 4-n  <=>  rank > 3-n  (unrolled by the compiler)
                let thr = vdupq_n_u32(3 - n);
                let mi = vcgtq_u32(rx, thr);
                let mj = vcgtq_u32(ry, thr);
                let mk = vcgtq_u32(rz, thr);
                let ml = vcgtq_u32(rw, thr);
                let h = vaddq_u32(
                    vaddq_u32(
                        vaddq_u32(hb, vandq_u32(mi, vdupq_n_u32(super::P1))),
                        vandq_u32(mj, vdupq_n_u32(super::P2)),
                    ),
                    vaddq_u32(
                        vandq_u32(mk, vdupq_n_u32(super::P3)),
                        vandq_u32(ml, vdupq_n_u32(super::P4)),
                    ),
                );
                let g = vdupq_n_f32(n as f32 * super::G4);
                let dx = vaddq_f32(vsubq_f32(x0, vreinterpretq_f32_u32(vandq_u32(mi, onef))), g);
                let dy = vaddq_f32(vsubq_f32(y0, vreinterpretq_f32_u32(vandq_u32(mj, onef))), g);
                let dz = vaddq_f32(vsubq_f32(z0, vreinterpretq_f32_u32(vandq_u32(mk, onef))), g);
                let dw = vaddq_f32(vsubq_f32(w0, vreinterpretq_f32_u32(vandq_u32(ml, onef))), g);
                sum = vaddq_f32(sum, corner(h, dx, dy, dz, dw));
            }

            // Last corner: all offsets are 1.
            let g4m1 = vdupq_n_f32(4.0 * super::G4 - 1.0);
            sum = vaddq_f32(
                sum,
                corner(
                    vaddq_u32(hb, vdupq_n_u32(super::P1234)),
                    vaddq_f32(x0, g4m1),
                    vaddq_f32(y0, g4m1),
                    vaddq_f32(z0, g4m1),
                    vaddq_f32(w0, g4m1),
                ),
            );

            let r = vmulq_f32(sum, vdupq_n_f32(super::AMP));
            let mut out = [0f32; 4];
            vst1q_f32(out.as_mut_ptr(), r);
            out
        }
    }
}

/// Portable fallback (x86_64, wasm, …): scalar code mirroring the NEON path
/// operation-for-operation in the same association order, so the field is
/// bit-identical across platforms. Still branchless and table-free; LLVM
/// frequently re-vectorizes the 4-lane caller loop.
#[cfg(not(all(target_arch = "aarch64", target_feature = "neon")))]
mod lanes {
    /// `vbicq`-style: keep `v`'s bits unless `zeroed`.
    #[inline(always)]
    fn keep_unless(v: f32, zeroed: bool) -> f32 {
        if zeroed {
            0.0
        } else {
            v
        }
    }

    #[inline(always)]
    fn corner(h: u32, dx: f32, dy: f32, dz: f32, dw: f32) -> f32 {
        let h = h ^ (h >> 15);
        let h = h.wrapping_mul(super::MIX);
        let h = h ^ (h >> 13);
        let sign = 0x8000_0000u32;
        let gx = f32::from_bits(dx.to_bits() ^ ((h << 6) & sign));
        let gy = f32::from_bits(dy.to_bits() ^ ((h << 5) & sign));
        let gz = f32::from_bits(dz.to_bits() ^ ((h << 4) & sign));
        let gw = f32::from_bits(dw.to_bits() ^ ((h << 3) & sign));
        let zi = (h >> 29) & 3;
        let gd = (keep_unless(gx, zi == 0) + keep_unless(gy, zi == 1))
            + (keep_unless(gz, zi == 2) + keep_unless(gw, zi == 3));
        let tt = super::R2 - dx * dx - dy * dy - dz * dz - dw * dw;
        let tt = tt.max(0.0);
        let tsq = tt * tt;
        (tsq * tsq) * gd
    }

    #[inline(always)]
    fn eval1(seed: u32, x: f32, y: f32, z: f32, w: f32, zw: f32) -> f32 {
        let s = ((x + y) + zw) * super::F4;
        let fi = (x + s).floor();
        let fj = (y + s).floor();
        let fk = (z + s).floor();
        let fl = (w + s).floor();
        let ii = fi as i32 as u32;
        let jj = fj as i32 as u32;
        let kk = fk as i32 as u32;
        let ll = fl as i32 as u32;
        let t = (((fi + fj) + fk) + fl) * super::G4;
        let x0 = (x - fi) + t;
        let y0 = (y - fj) + t;
        let z0 = (z - fk) + t;
        let w0 = (w - fl) + t;

        let cxy = (x0 > y0) as u32;
        let cxz = (x0 > z0) as u32;
        let cxw = (x0 > w0) as u32;
        let cyz = (y0 > z0) as u32;
        let cyw = (y0 > w0) as u32;
        let czw = (z0 > w0) as u32;
        let rx = cxy + cxz + cxw;
        let ry = (cxy ^ 1) + cyz + cyw;
        let rz = (cxz ^ 1) + (cyz ^ 1) + czw;
        let rw = (cxw ^ 1) + (cyw ^ 1) + (czw ^ 1);

        let hb = ii
            .wrapping_mul(super::P1)
            .wrapping_add(jj.wrapping_mul(super::P2))
            .wrapping_add(
                kk.wrapping_mul(super::P3)
                    .wrapping_add(ll.wrapping_mul(super::P4)),
            )
            .wrapping_add(seed);

        let mut sum = corner(hb, x0, y0, z0, w0);
        for n in 1..=3u32 {
            let thr = 3 - n;
            let mi = rx > thr;
            let mj = ry > thr;
            let mk = rz > thr;
            let ml = rw > thr;
            let h = hb
                .wrapping_add(if mi { super::P1 } else { 0 })
                .wrapping_add(if mj { super::P2 } else { 0 })
                .wrapping_add(if mk { super::P3 } else { 0 })
                .wrapping_add(if ml { super::P4 } else { 0 });
            let g = n as f32 * super::G4;
            let dx = (x0 - if mi { 1.0 } else { 0.0 }) + g;
            let dy = (y0 - if mj { 1.0 } else { 0.0 }) + g;
            let dz = (z0 - if mk { 1.0 } else { 0.0 }) + g;
            let dw = (w0 - if ml { 1.0 } else { 0.0 }) + g;
            sum += corner(h, dx, dy, dz, dw);
        }

        let g4m1 = 4.0 * super::G4 - 1.0;
        sum += corner(
            hb.wrapping_add(super::P1234),
            x0 + g4m1,
            y0 + g4m1,
            z0 + g4m1,
            w0 + g4m1,
        );
        sum * super::AMP
    }

    #[inline(always)]
    pub fn eval4(seed: u32, xs: &[f32; 4], ys: &[f32; 4], z: f32, w: f32) -> [f32; 4] {
        let zw = z + w;
        let mut out = [0f32; 4];
        for l in 0..4 {
            out[l] = eval1(seed, xs[l], ys[l], z, w, zw);
        }
        out
    }
}

struct SimdSimplex4 {
    seed: u32,
}

impl Noise4 for SimdSimplex4 {
    fn name(&self) -> &'static str {
        "simd: 4D simplex, NEON 4-lane, computed hash"
    }

    fn get(&self, point: [f64; 4]) -> f64 {
        // Splat the point across all lanes and read lane 0: same code path
        // as rows, still well under the baseline's per-point cost.
        let x = (point[0] * FREQ) as f32;
        let y = (point[1] * FREQ) as f32;
        let z = (point[2] * FREQ) as f32;
        let w = (point[3] * FREQ) as f32;
        lanes::eval4(self.seed, &[x; 4], &[y; 4], z, w)[0] as f64
    }

    fn get_row(&self, x_pairs: &[(f64, f64)], z_pair: (f64, f64), out: &mut [f64]) {
        let z = (z_pair.0 * FREQ) as f32;
        let w = (z_pair.1 * FREQ) as f32;
        let n = x_pairs.len().min(out.len());
        let mut i = 0;
        while i < n {
            let take = (n - i).min(4);
            // Partial final block: pad with the last point (extra lanes are
            // computed but not written back).
            let mut xa = [(x_pairs[i + take - 1].0 * FREQ) as f32; 4];
            let mut ya = [(x_pairs[i + take - 1].1 * FREQ) as f32; 4];
            for l in 0..take {
                xa[l] = (x_pairs[i + l].0 * FREQ) as f32;
                ya[l] = (x_pairs[i + l].1 * FREQ) as f32;
            }
            let r = lanes::eval4(self.seed, &xa, &ya, z, w);
            for (o, v) in out[i..i + take].iter_mut().zip(r) {
                *o = v as f64;
            }
            i += take;
        }
    }
}
