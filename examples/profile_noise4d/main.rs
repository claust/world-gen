//! Microbenchmark for the 4D-noise hot path of chunk generation.
//!
//! Initial chunk generation is dominated by 4D OpenSimplex evaluations in the
//! `noise` crate: one detail-octave eval per terrain vertex (129×129 per
//! chunk) plus the one-time `TerrainFields`/`RiverField` bakes, which sample
//! the same way. This harness isolates exactly that call pattern — points on
//! a torus embedded in 4D, with the per-axis trig hoisted as in
//! `Heightmap::height_grid` — and times candidate implementations against the
//! `noise` crate baseline over a simulated batch of chunks.
//!
//! Run:
//!   cargo run --release --example profile_noise4d
//!   PROFILE_CHUNKS=400 cargo run --release --example profile_noise4d
//!   NOISE_PNG=1 cargo run --release --example profile_noise4d   # dump terrain-character PNGs
//!
//! Candidate implementations live in sibling modules (`faithful.rs`, `os2.rs`,
//! `simd.rs`), each exposing `create(seed) -> Option<Box<dyn Noise4>>`. A stub
//! returning `None` is skipped by the harness, so candidates can land
//! independently.

mod faithful;
mod os2;
mod simd;

use std::f64::consts::TAU;
use std::time::Instant;

use noise::{NoiseFn, OpenSimplex};
use rayon::prelude::*;

use world_gen::world_core::chunk::{CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS, WORLD_SIZE_METERS};

const SIDE: usize = CHUNK_GRID_RESOLUTION;
const EVALS_PER_CHUNK: usize = SIDE * SIDE;
/// Detail-octave parameters from `HeightmapConfig::default()`; the seed offset
/// matches `Heightmap::new`.
const DETAIL_FREQUENCY: f64 = 0.018;
const SEED: u32 = 42u32.wrapping_add(907);

/// The interface a 4D-noise candidate must implement.
///
/// `get` is the drop-in point API (`OpenSimplex::get` shape). `get_row` is the
/// shape `Heightmap::height_grid` actually consumes: one row of vertices
/// shares the z circle-pair while the x circle-pairs vary — batched
/// implementations (SIMD) should override it; the default just loops `get`.
pub trait Noise4: Sync {
    fn name(&self) -> &'static str;
    fn get(&self, point: [f64; 4]) -> f64;
    fn get_row(&self, x_pairs: &[(f64, f64)], z_pair: (f64, f64), out: &mut [f64]) {
        for (o, x) in out.iter_mut().zip(x_pairs) {
            *o = self.get([x.0, x.1, z_pair.0, z_pair.1]);
        }
    }
}

/// Baseline: the `noise` crate's OpenSimplex, exactly as production uses it.
struct CrateOpenSimplex(OpenSimplex);

impl Noise4 for CrateOpenSimplex {
    fn name(&self) -> &'static str {
        "noise crate OpenSimplex (baseline)"
    }
    fn get(&self, point: [f64; 4]) -> f64 {
        self.0.get(point)
    }
}

/// The torus mapping from `Heightmap::torus_sample`, specialized to the
/// detail octave: world axis -> circle (radius·cosθ, radius·sinθ).
struct TorusMap {
    radius: f64,
}

impl TorusMap {
    fn detail() -> Self {
        let k = (DETAIL_FREQUENCY * WORLD_SIZE_METERS).round().max(1.0);
        Self { radius: k / TAU }
    }
    fn pair(&self, w: f64) -> (f64, f64) {
        let theta = TAU * w / WORLD_SIZE_METERS;
        (self.radius * theta.cos(), self.radius * theta.sin())
    }
    /// Hoisted per-axis trig for a chunk-sized grid, as `height_grid` does.
    fn axis_pairs(&self, origin: f64, cell: f64, side: usize) -> Vec<(f64, f64)> {
        (0..side)
            .map(|i| self.pair(origin + i as f64 * cell))
            .collect()
    }
}

/// Deterministically scattered chunk origins across the whole wrapping world,
/// so the workload isn't biased to one neighbourhood of the noise domain.
fn chunk_origins(n: usize) -> Vec<(f64, f64)> {
    let world_chunks = (WORLD_SIZE_METERS / CHUNK_SIZE_METERS as f64) as u64;
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) % world_chunks
    };
    (0..n)
        .map(|_| {
            (
                next() as f64 * CHUNK_SIZE_METERS as f64,
                next() as f64 * CHUNK_SIZE_METERS as f64,
            )
        })
        .collect()
}

/// One simulated chunk of detail-octave work via the point API.
fn chunk_points(noise: &dyn Noise4, torus: &TorusMap, origin: (f64, f64), cell: f64) -> f64 {
    let xs = torus.axis_pairs(origin.0, cell, SIDE);
    let zs = torus.axis_pairs(origin.1, cell, SIDE);
    let mut acc = 0.0;
    for z in &zs {
        for x in &xs {
            acc += noise.get([x.0, x.1, z.0, z.1]);
        }
    }
    acc
}

/// One simulated chunk of detail-octave work via the row API.
fn chunk_rows(noise: &dyn Noise4, torus: &TorusMap, origin: (f64, f64), cell: f64) -> f64 {
    let xs = torus.axis_pairs(origin.0, cell, SIDE);
    let zs = torus.axis_pairs(origin.1, cell, SIDE);
    let mut row = vec![0.0f64; SIDE];
    let mut acc = 0.0;
    for z in &zs {
        noise.get_row(&xs, *z, &mut row);
        acc += row.iter().sum::<f64>();
    }
    acc
}

struct BenchResult {
    label: &'static str,
    ms_per_chunk: f64,
    ns_per_eval: f64,
}

fn bench(
    label: &'static str,
    origins: &[(f64, f64)],
    reps: usize,
    f: impl Fn(&[(f64, f64)]) -> f64,
) -> BenchResult {
    // warmup
    std::hint::black_box(f(&origins[..origins.len().min(8)]));
    let mut best = f64::INFINITY;
    for _ in 0..reps {
        let t = Instant::now();
        std::hint::black_box(f(origins));
        best = best.min(t.elapsed().as_secs_f64());
    }
    let evals = origins.len() * EVALS_PER_CHUNK;
    BenchResult {
        label,
        ms_per_chunk: best * 1e3 / origins.len() as f64,
        ns_per_eval: best * 1e9 / evals as f64,
    }
}

/// Value statistics + smoothness proxy over the realistic workload, used both
/// to sanity-check candidates and to compare their "character" to baseline:
/// `std` calibrates amplitude, `roughness` (mean |Δ| between 2 m-spaced
/// neighbours, relative to std) proxies the spatial frequency content.
struct Quality {
    mean: f64,
    std: f64,
    min: f64,
    max: f64,
    roughness: f64,
    /// Max abs difference vs the baseline noise at identical inputs (tells a
    /// faithful port apart from a different-character replacement).
    max_diff_vs_baseline: f64,
}

fn quality(noise: &dyn Noise4, baseline: &dyn Noise4, torus: &TorusMap, cell: f64) -> Quality {
    let origins = chunk_origins(8);
    let mut values = Vec::with_capacity(origins.len() * EVALS_PER_CHUNK);
    let mut delta_sum = 0.0f64;
    let mut deltas = 0u64;
    let mut max_diff = 0.0f64;
    for o in &origins {
        let xs = torus.axis_pairs(o.0, cell, SIDE);
        let zs = torus.axis_pairs(o.1, cell, SIDE);
        for z in &zs {
            let mut prev = None;
            for x in &xs {
                let p = [x.0, x.1, z.0, z.1];
                let v = noise.get(p);
                max_diff = max_diff.max((v - baseline.get(p)).abs());
                if let Some(pv) = prev {
                    delta_sum += f64::abs(v - pv);
                    deltas += 1;
                }
                prev = Some(v);
                values.push(v);
            }
        }
    }
    let n = values.len() as f64;
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n;
    let std = var.sqrt();
    let (min, max) = values
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(*v), hi.max(*v))
        });
    Quality {
        mean,
        std,
        min,
        max,
        roughness: (delta_sum / deltas as f64) / std,
        max_diff_vs_baseline: max_diff,
    }
}

/// Grayscale render of the noise over a 4-chunk-wide region at terrain vertex
/// spacing, for eyeballing terrain character side by side.
fn dump_png(noise: &dyn Noise4, torus: &TorusMap, cell: f64, path: &str) {
    const RES: usize = 512;
    let xs = torus.axis_pairs(12_800.0, cell, RES);
    let zs = torus.axis_pairs(12_800.0, cell, RES);
    let mut values = vec![0.0f64; RES * RES];
    for (zi, z) in zs.iter().enumerate() {
        noise.get_row(&xs, *z, &mut values[zi * RES..(zi + 1) * RES]);
    }
    let max_abs = values.iter().fold(0.0f64, |m, v| m.max(v.abs())).max(1e-9);
    let pixels: Vec<u8> = values
        .iter()
        .map(|v| ((v / max_abs) * 127.0 + 128.0) as u8)
        .collect();
    image::GrayImage::from_raw(RES as u32, RES as u32, pixels)
        .expect("buffer size matches dimensions")
        .save(path)
        .expect("write PNG");
    println!("  wrote {path}");
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let n_chunks = env_usize("PROFILE_CHUNKS", 100);
    let reps = env_usize("PROFILE_REPS", 3);
    let dump = std::env::var("NOISE_PNG").is_ok();
    let origins = chunk_origins(n_chunks);
    let torus = TorusMap::detail();
    let cell = (CHUNK_SIZE_METERS / (SIDE - 1) as f32) as f64;

    println!("=== 4D noise microbenchmark (detail-octave call pattern) ===");
    println!(
        "chunks: {n_chunks}  ({SIDE}×{SIDE} = {EVALS_PER_CHUNK} evals/chunk, {} evals total)",
        n_chunks * EVALS_PER_CHUNK
    );
    println!(
        "torus radius: {:.1}  (k = {} cycles/world)\n",
        torus.radius,
        (DETAIL_FREQUENCY * WORLD_SIZE_METERS).round()
    );

    let baseline: Box<dyn Noise4> = Box::new(CrateOpenSimplex(OpenSimplex::new(SEED)));
    let mut candidates: Vec<Box<dyn Noise4>> = vec![];
    candidates.extend(faithful::create(SEED));
    candidates.extend(os2::create(SEED));
    candidates.extend(simd::create(SEED));

    let base_point = bench("point API", &origins, reps, |o| {
        o.iter()
            .map(|o| chunk_points(baseline.as_ref(), &torus, *o, cell))
            .sum()
    });
    let mut reference_ns = base_point.ns_per_eval;

    for noise in std::iter::once(&baseline).chain(candidates.iter()) {
        let noise = noise.as_ref();
        println!("--- {} ---", noise.name());

        let q = quality(noise, baseline.as_ref(), &torus, cell);
        println!(
            "  quality : mean {:+.4}  std {:.4}  range [{:+.3}, {:+.3}]  roughness {:.4}",
            q.mean, q.std, q.min, q.max, q.roughness
        );
        println!(
            "  fidelity: max |diff| vs baseline = {:.3e}",
            q.max_diff_vs_baseline
        );

        let point = bench("point API", &origins, reps, |o| {
            o.iter()
                .map(|o| chunk_points(noise, &torus, *o, cell))
                .sum()
        });
        let row = bench("row API  ", &origins, reps, |o| {
            o.iter().map(|o| chunk_rows(noise, &torus, *o, cell)).sum()
        });
        let par = bench("row, par ", &origins, reps, |o| {
            o.par_iter()
                .map(|o| chunk_rows(noise, &torus, *o, cell))
                .sum()
        });
        for r in [&point, &row] {
            println!(
                "  {} : {:>7.3} ms/chunk  {:>7.1} ns/eval  ({:.2}× vs baseline point API)",
                r.label,
                r.ms_per_chunk,
                r.ns_per_eval,
                reference_ns / r.ns_per_eval
            );
        }
        println!(
            "  {} : {:>7.3} ms/chunk  (rayon over chunks, {:.0} chunks/s)",
            par.label,
            par.ms_per_chunk,
            1e3 / par.ms_per_chunk
        );
        if noise.name().contains("baseline") {
            reference_ns = point.ns_per_eval;
        }

        if dump {
            std::fs::create_dir_all("target/noise4d").expect("create output dir");
            let slug: String = noise
                .name()
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect();
            dump_png(noise, &torus, cell, &format!("target/noise4d/{slug}.png"));
        }
        println!();
    }

    if candidates.is_empty() {
        println!("(no candidate implementations yet — fill in faithful.rs / os2.rs / simd.rs)");
    }
}
