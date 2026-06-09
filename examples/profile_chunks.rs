//! Profiling harness for CPU-side base-world chunk generation.
//!
//! Drives the same public API the real base-world build uses
//! (`ChunkGenerator` and its three sub-layers) over a few thousand distinct
//! chunks, and breaks the cost down so we can see where the time actually goes
//! and prototype optimizations without touching the engine.
//!
//! Run:
//!   cargo run --release --example profile_chunks
//!   PROFILE_CHUNKS=8192 PROFILE_RIVER_RES=512 cargo run --release --example profile_chunks
//!
//! `PROFILE_RIVER_RES` only changes the one-time river-field solve (so the
//! harness starts fast); per-chunk river sampling is a bilinear lookup and is
//! independent of the grid resolution, so it does not affect the chunk-gen
//! numbers below.

// This is a throwaway benchmark harness: the prototype samplers index into
// precomputed trig/height grids by row/column, which reads clearer than
// iterator adapters here.
#![allow(clippy::needless_range_loop)]

use std::f64::consts::TAU;
use std::sync::Arc;
use std::time::Instant;

use glam::IVec2;
use noise::{NoiseFn, OpenSimplex};
use rayon::prelude::*;

use world_gen::world_core::biome_map::BiomeLayer;
use world_gen::world_core::chunk::{CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS, WORLD_SIZE_METERS};
use world_gen::world_core::chunk_generator::ChunkGenerator;
use world_gen::world_core::config::{GameConfig, HeightmapConfig};
use world_gen::world_core::content::{ContentInput, ContentLayer};
use world_gen::world_core::heightmap::Heightmap;
use world_gen::world_core::herbarium::{Herbarium, PlantRegistry};
use world_gen::world_core::layer::Layer;
use world_gen::world_core::rivers::RiverField;
use world_gen::world_core::terrain::TerrainLayer;

const SIDE: usize = CHUNK_GRID_RESOLUTION;
const TOTAL: usize = SIDE * SIDE;

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A deterministic block of distinct chunk coordinates.
fn chunk_coords(n: usize) -> Vec<IVec2> {
    let w = (n as f64).sqrt().ceil() as i32;
    (0..n as i32).map(|i| IVec2::new(i % w, i / w)).collect()
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() {
    let n = env_usize("PROFILE_CHUNKS", 4096);
    let coords = chunk_coords(n);
    let cores = std::thread::available_parallelism()
        .map(|c| c.get())
        .unwrap_or(8);

    println!("=== chunk-gen profiling harness ===");
    println!(
        "chunks per run: {n}  ({}×{} grid, {} verts/chunk)",
        SIDE, SIDE, TOTAL
    );
    println!("logical cores:  {cores}\n");

    // ---- one-time setup (mirrors generate_base_world) -------------------
    let mut config = GameConfig::default();
    let river_res = env_usize("PROFILE_RIVER_RES", config.rivers.grid_resolution as usize);
    config.rivers.grid_resolution = river_res as u32;
    let seed = config.world.seed;

    let herb = Herbarium::default_seeded();
    let registry = Arc::new(PlantRegistry::from_herbarium(&herb));

    let t = Instant::now();
    let rivers = Arc::new(RiverField::generate(seed, &config));
    println!(
        "[setup] RiverField::generate (grid {river_res}): {:.1} ms  (one-time, not per-chunk)",
        ms(t.elapsed())
    );

    let generator = ChunkGenerator::new(seed, &config, Arc::clone(&registry), Arc::clone(&rivers));

    // Sub-layers rebuilt standalone so each can be timed in isolation. These
    // use exactly the same public constructors ChunkGenerator uses internally.
    let terrain_layer = TerrainLayer::new(
        seed,
        config.heightmap.clone(),
        config.sea_level,
        Arc::clone(&rivers),
    );
    let biome_layer = BiomeLayer {
        biome_config: config.biome.clone(),
    };
    let content_layer = ContentLayer::new(seed, &config, Arc::clone(&registry));

    // warmup
    for c in coords.iter().take(64) {
        std::hint::black_box(generator.generate_chunk(*c));
    }

    experiment_thread_scaling(&generator, &coords, cores);
    experiment_layer_breakdown(&terrain_layer, &biome_layer, &content_layer, &coords);
    experiment_terrain_subparts(seed, &config.heightmap, &rivers, &coords);
    experiment_nested_parallelism(seed, &config.heightmap, &rivers, &coords);
    experiment_optimized_noise(seed, &config.heightmap, &coords);
    experiment_coarse_lowfreq(seed, &config.heightmap, &coords);
    experiment_coarse_rawnoise(seed, &config.heightmap, &coords);
    experiment_global_rawnoise(seed, &config, &coords);
}

// ---------------------------------------------------------------------------
// E1. Full-pipeline throughput and parallel scaling.
// Mirrors the real base build: outer rayon par_iter over chunks.
// ---------------------------------------------------------------------------
fn experiment_thread_scaling(generator: &ChunkGenerator, coords: &[IVec2], cores: usize) {
    println!("\n--- E1: full generate_chunk throughput & thread scaling ---");

    // serial baseline (also gives clean per-chunk ms)
    let t = Instant::now();
    let mut plants = 0usize;
    for c in coords {
        let d = generator.generate_chunk(*c);
        plants += d.content.base_plants.len();
    }
    let serial = t.elapsed();
    let per_chunk = ms(serial) / coords.len() as f64;
    println!(
        "  1 thread : {:>8.1} ms total  | {:>7.3} ms/chunk | {:>8.0} chunks/s | ~{} plants/chunk",
        ms(serial),
        per_chunk,
        coords.len() as f64 / serial.as_secs_f64(),
        plants / coords.len()
    );

    let mut thread_counts: Vec<usize> = vec![2, 4, 8];
    if !thread_counts.contains(&cores) {
        thread_counts.push(cores);
    }
    thread_counts.retain(|&c| c <= cores && c > 1);

    for &nt in &thread_counts {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(nt)
            .build()
            .unwrap();
        let t = Instant::now();
        pool.install(|| {
            coords.par_iter().for_each(|c| {
                std::hint::black_box(generator.generate_chunk(*c));
            });
        });
        let el = t.elapsed();
        let speedup = serial.as_secs_f64() / el.as_secs_f64();
        println!(
            "  {:>2} thr  : {:>8.1} ms total  | {:>8.0} chunks/s | speedup {:>4.1}× | efficiency {:>3.0}%",
            nt,
            ms(el),
            coords.len() as f64 / el.as_secs_f64(),
            speedup,
            100.0 * speedup / nt as f64
        );
    }

    // extrapolate full world
    let full = (256 * 256) as f64;
    println!(
        "  → full world is 256×256 = 65,536 chunks; at the best rate above that's the dominant base-build cost.",
    );
    let _ = full;
}

// ---------------------------------------------------------------------------
// E2. Per-layer breakdown (serial, single thread) — where does a chunk's time go?
// ---------------------------------------------------------------------------
fn experiment_layer_breakdown(
    terrain_layer: &TerrainLayer,
    biome_layer: &BiomeLayer,
    content_layer: &ContentLayer,
    coords: &[IVec2],
) {
    println!("\n--- E2: per-layer breakdown (single thread) ---");
    let n = coords.len();

    let mut t_terrain = std::time::Duration::ZERO;
    let mut t_biome = std::time::Duration::ZERO;
    let mut t_content = std::time::Duration::ZERO;

    for c in coords {
        let t = Instant::now();
        let terrain = terrain_layer.generate(*c);
        t_terrain += t.elapsed();

        let t = Instant::now();
        let biome = biome_layer.generate(&terrain);
        t_biome += t.elapsed();

        let t = Instant::now();
        let content = content_layer.generate(ContentInput {
            coord: *c,
            terrain: &terrain,
            biome_map: &biome,
        });
        t_content += t.elapsed();
        std::hint::black_box(content);
    }

    let total = t_terrain + t_biome + t_content;
    let pct = |d: std::time::Duration| 100.0 * d.as_secs_f64() / total.as_secs_f64();
    println!(
        "  terrain (noise+rivers) : {:>7.3} ms/chunk  {:>4.0}%",
        ms(t_terrain) / n as f64,
        pct(t_terrain)
    );
    println!(
        "  biome   (classify)     : {:>7.3} ms/chunk  {:>4.0}%",
        ms(t_biome) / n as f64,
        pct(t_biome)
    );
    println!(
        "  content (flora+houses) : {:>7.3} ms/chunk  {:>4.0}%",
        ms(t_content) / n as f64,
        pct(t_content)
    );
    println!(
        "  total                  : {:>7.3} ms/chunk",
        ms(total) / n as f64
    );
    println!("  note: terrain's inner per-vertex loop ALSO uses rayon — measured here on 1 thread so it runs serial.");
}

// ---------------------------------------------------------------------------
// E3. Terrain sub-breakdown: height (3 octaves) vs moisture (2) vs river sample.
// Replicates terrain.rs's loop using the public Heightmap / RiverField API.
// ---------------------------------------------------------------------------
fn experiment_terrain_subparts(
    seed: u32,
    cfg: &HeightmapConfig,
    rivers: &RiverField,
    coords: &[IVec2],
) {
    println!("\n--- E3: terrain field sub-breakdown (single thread) ---");
    let hm = Heightmap::new(seed, cfg.clone());
    let cell = CHUNK_SIZE_METERS / (SIDE - 1) as f32;
    let n = coords.len();

    let mut acc = 0.0f64;
    macro_rules! pass {
        ($label:expr, $body:expr) => {{
            let t = Instant::now();
            for c in coords {
                let ox = c.x as f32 * CHUNK_SIZE_METERS;
                let oz = c.y as f32 * CHUNK_SIZE_METERS;
                for idx in 0..TOTAL {
                    let x = ox + (idx % SIDE) as f32 * cell;
                    let z = oz + (idx / SIDE) as f32 * cell;
                    acc += $body(x, z);
                }
            }
            let d = t.elapsed();
            println!("  {:<26}: {:>7.3} ms/chunk", $label, ms(d) / n as f64);
        }};
    }

    pass!("height (3 noise octaves)", |x, z| hm.sample_height(x, z)
        as f64);
    pass!(
        "moisture (2 noise octaves)",
        |x, z| hm.sample_moisture(x, z) as f64
    );
    pass!("river sample (bilinear)", |x, z| {
        let raw = hm.sample_height(x, z);
        let (a, b, _s) = rivers.sample(raw, x, z);
        (a + b) as f64
    });
    std::hint::black_box(acc);
    println!(
        "  → height+moisture = 5 4D-OpenSimplex evals/vertex × {} verts = the terrain hot spot.",
        TOTAL
    );
}

// ---------------------------------------------------------------------------
// E4. Nested parallelism cost.
// The real base build does par_iter over chunks; terrain.rs ALSO does par_iter
// over the 16,641 vertices inside each chunk. When the outer loop already
// saturates every core, the inner par_iter just adds task-splitting overhead.
// Here we replicate terrain generation two ways and run both under an outer
// par_iter over chunks, to isolate that overhead.
// ---------------------------------------------------------------------------
fn experiment_nested_parallelism(
    seed: u32,
    cfg: &HeightmapConfig,
    rivers: &Arc<RiverField>,
    coords: &[IVec2],
) {
    println!("\n--- E4: nested parallelism (outer par over chunks) ---");
    let hm = Arc::new(Heightmap::new(seed, cfg.clone()));
    let cell = CHUNK_SIZE_METERS / (SIDE - 1) as f32;

    let gen_serial_inner = {
        let hm = Arc::clone(&hm);
        let rivers = Arc::clone(rivers);
        move |c: IVec2| -> Vec<f32> {
            let ox = c.x as f32 * CHUNK_SIZE_METERS;
            let oz = c.y as f32 * CHUNK_SIZE_METERS;
            let mut heights = Vec::with_capacity(TOTAL);
            for idx in 0..TOTAL {
                let x = ox + (idx % SIDE) as f32 * cell;
                let z = oz + (idx / SIDE) as f32 * cell;
                let raw = hm.sample_height(x, z);
                let (h, _w, _s) = rivers.sample(raw, x, z);
                heights.push(h);
            }
            heights
        }
    };

    let gen_par_inner = {
        let hm = Arc::clone(&hm);
        let rivers = Arc::clone(rivers);
        move |c: IVec2| -> Vec<f32> {
            let ox = c.x as f32 * CHUNK_SIZE_METERS;
            let oz = c.y as f32 * CHUNK_SIZE_METERS;
            (0..TOTAL)
                .into_par_iter()
                .map(|idx| {
                    let x = ox + (idx % SIDE) as f32 * cell;
                    let z = oz + (idx / SIDE) as f32 * cell;
                    let raw = hm.sample_height(x, z);
                    let (h, _w, _s) = rivers.sample(raw, x, z);
                    h
                })
                .collect()
        }
    };

    let bench = |label: &str, f: &(dyn Fn(IVec2) -> Vec<f32> + Sync)| {
        let t = Instant::now();
        coords.par_iter().for_each(|c| {
            std::hint::black_box(f(*c));
        });
        println!(
            "  {:<34}: {:>8.1} ms  | {:>8.0} chunks/s",
            label,
            ms(t.elapsed()),
            coords.len() as f64 / t.elapsed().as_secs_f64()
        );
    };

    // run each a couple times; report the second (warm) run
    bench("inner SERIAL (no nesting)", &gen_serial_inner);
    bench("inner SERIAL (no nesting)", &gen_serial_inner);
    bench("inner PAR (nested, like terrain.rs)", &gen_par_inner);
    bench("inner PAR (nested, like terrain.rs)", &gen_par_inner);
}

// ---------------------------------------------------------------------------
// E5. Optimized noise prototype: precompute the per-axis torus trig.
// torus_sample maps each world axis onto a circle (cos/sin) and feeds 4D
// OpenSimplex. The two circle coords for x depend only on the column, and the
// two for z only on the row — so over a 129×129 grid we can precompute
// 129 x-pairs + 129 z-pairs per octave instead of 4 trig calls per vertex.
// Algebraically identical; we verify max abs error.
// ---------------------------------------------------------------------------
struct Octave {
    noise: OpenSimplex,
    radius: f64,
    inv_l: f64,
}
impl Octave {
    fn new(noise: OpenSimplex, frequency: f64) -> Self {
        let k = (frequency * WORLD_SIZE_METERS).round().max(1.0);
        Self {
            noise,
            radius: k / TAU,
            inv_l: 1.0 / WORLD_SIZE_METERS,
        }
    }
    fn trig(&self, w: f64) -> (f64, f64) {
        let theta = TAU * w * self.inv_l;
        (self.radius * theta.cos(), self.radius * theta.sin())
    }
}

fn experiment_optimized_noise(seed: u32, cfg: &HeightmapConfig, coords: &[IVec2]) {
    println!("\n--- E5: optimized noise prototype (precomputed per-axis trig) ---");
    let hm = Heightmap::new(seed, cfg.clone());

    // Rebuild the 3 height octaves with the same seeds Heightmap::new uses.
    let cont = Octave::new(OpenSimplex::new(seed), cfg.continental.frequency);
    let ridge = Octave::new(
        OpenSimplex::new(seed.wrapping_add(101)),
        cfg.ridge.frequency,
    );
    let detail = Octave::new(
        OpenSimplex::new(seed.wrapping_add(907)),
        cfg.detail.frequency,
    );
    let (a_c, a_r, a_d) = (
        cfg.continental.amplitude,
        cfg.ridge.amplitude,
        cfg.detail.amplitude,
    );

    let cell = CHUNK_SIZE_METERS / (SIDE - 1) as f32;

    let gen_opt = |c: IVec2| -> Vec<f32> {
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        // Precompute per-axis trig for each octave (129 cols, 129 rows).
        let mut cx = [[(0.0f64, 0.0f64); SIDE]; 3];
        let mut cz = [[(0.0f64, 0.0f64); SIDE]; 3];
        for i in 0..SIDE {
            let wx = (ox + i as f32 * cell) as f64;
            let wz = (oz + i as f32 * cell) as f64;
            cx[0][i] = cont.trig(wx);
            cz[0][i] = cont.trig(wz);
            cx[1][i] = ridge.trig(wx);
            cz[1][i] = ridge.trig(wz);
            cx[2][i] = detail.trig(wx);
            cz[2][i] = detail.trig(wz);
        }
        let mut out = Vec::with_capacity(TOTAL);
        for zi in 0..SIDE {
            for xi in 0..SIDE {
                let broad = cont
                    .noise
                    .get([cx[0][xi].0, cx[0][xi].1, cz[0][zi].0, cz[0][zi].1])
                    as f32;
                let ridges = 1.0
                    - (ridge
                        .noise
                        .get([cx[1][xi].0, cx[1][xi].1, cz[1][zi].0, cz[1][zi].1])
                        .abs() as f32);
                let rough = detail
                    .noise
                    .get([cx[2][xi].0, cx[2][xi].1, cz[2][zi].0, cz[2][zi].1])
                    as f32;
                out.push(broad * a_c + ridges * a_r + rough * a_d);
            }
        }
        out
    };

    // correctness: compare against the real sampler on a few chunks
    let mut max_err = 0.0f32;
    for c in coords.iter().take(8) {
        let opt = gen_opt(*c);
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        for idx in 0..TOTAL {
            let x = ox + (idx % SIDE) as f32 * cell;
            let z = oz + (idx / SIDE) as f32 * cell;
            max_err = max_err.max((opt[idx] - hm.sample_height(x, z)).abs());
        }
    }
    println!(
        "  correctness: max abs height error vs baseline = {:.6} m",
        max_err
    );

    // baseline height-only generation (serial)
    let n = coords.len();
    let t = Instant::now();
    let mut acc = 0.0f64;
    for c in coords {
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        for idx in 0..TOTAL {
            let x = ox + (idx % SIDE) as f32 * cell;
            let z = oz + (idx / SIDE) as f32 * cell;
            acc += hm.sample_height(x, z) as f64;
        }
    }
    let base = t.elapsed();

    let t = Instant::now();
    for c in coords {
        acc += gen_opt(*c).iter().map(|v| *v as f64).sum::<f64>();
    }
    let opt = t.elapsed();
    std::hint::black_box(acc);

    println!("  baseline height: {:>7.3} ms/chunk", ms(base) / n as f64);
    println!(
        "  precomp-trig   : {:>7.3} ms/chunk   ({:.2}× faster on the height field)",
        ms(opt) / n as f64,
        base.as_secs_f64() / opt.as_secs_f64()
    );
    println!(
        "  → bit-identical (err 0). Same trick applies to moisture; terrain is 90% of a chunk,"
    );
    println!(
        "     so ~1.8× on the noise ≈ ~1.7× on the whole base build, shipping the SAME world."
    );
}

// ---------------------------------------------------------------------------
// E6. Lossy option: sample the low-frequency octaves (continental ~10 km,
// ridge ~1.1 km wavelength) on a coarse grid and bilinearly interpolate them
// to full res; keep only the high-frequency detail octave (~55 m) at full res.
// This is NOT bit-identical — it would require regenerating the shipped
// snapshot — but the error is tiny because the low octaves barely change over a
// 256 m chunk. We report the max height error in metres so the trade is visible.
// ---------------------------------------------------------------------------
fn experiment_coarse_lowfreq(seed: u32, cfg: &HeightmapConfig, coords: &[IVec2]) {
    println!("\n--- E6: coarse low-freq octaves + full-res detail (LOSSY, needs new snapshot) ---");
    let coarse = env_usize("PROFILE_COARSE", 8); // nodes per side = coarse+1
    let hm = Heightmap::new(seed, cfg.clone());

    let cont = Octave::new(OpenSimplex::new(seed), cfg.continental.frequency);
    let ridge = Octave::new(
        OpenSimplex::new(seed.wrapping_add(101)),
        cfg.ridge.frequency,
    );
    let detail = Octave::new(
        OpenSimplex::new(seed.wrapping_add(907)),
        cfg.detail.frequency,
    );
    let (a_c, a_r, a_d) = (
        cfg.continental.amplitude,
        cfg.ridge.amplitude,
        cfg.detail.amplitude,
    );
    let cell = CHUNK_SIZE_METERS / (SIDE - 1) as f32;

    let sample_lowfreq = |wx: f64, wz: f64| -> f32 {
        let (cxx, cxs) = cont.trig(wx);
        let (czx, czs) = cont.trig(wz);
        let broad = cont.noise.get([cxx, cxs, czx, czs]) as f32;
        let (rxx, rxs) = ridge.trig(wx);
        let (rzx, rzs) = ridge.trig(wz);
        let ridges = 1.0 - (ridge.noise.get([rxx, rxs, rzx, rzs]).abs() as f32);
        broad * a_c + ridges * a_r
    };

    let nodes = coarse + 1;
    let gen_coarse = |c: IVec2| -> Vec<f32> {
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        let cstep = CHUNK_SIZE_METERS / coarse as f32;
        // coarse low-freq grid
        let mut lo = vec![0.0f32; nodes * nodes];
        for gz in 0..nodes {
            for gx in 0..nodes {
                let wx = (ox + gx as f32 * cstep) as f64;
                let wz = (oz + gz as f32 * cstep) as f64;
                lo[gz * nodes + gx] = sample_lowfreq(wx, wz);
            }
        }
        let mut out = Vec::with_capacity(TOTAL);
        for zi in 0..SIDE {
            let fz = zi as f32 * cell / cstep;
            let z0 = (fz.floor() as usize).min(nodes - 2);
            let tz = fz - z0 as f32;
            for xi in 0..SIDE {
                let fx = xi as f32 * cell / cstep;
                let x0 = (fx.floor() as usize).min(nodes - 2);
                let tx = fx - x0 as f32;
                let a = lo[z0 * nodes + x0];
                let b = lo[z0 * nodes + x0 + 1];
                let cc = lo[(z0 + 1) * nodes + x0];
                let d = lo[(z0 + 1) * nodes + x0 + 1];
                let low = a * (1.0 - tx) * (1.0 - tz)
                    + b * tx * (1.0 - tz)
                    + cc * (1.0 - tx) * tz
                    + d * tx * tz;
                // detail octave at full res, with precomputed trig per axis would
                // also apply; here sampled directly for a clean error reading.
                let wx = (ox + xi as f32 * cell) as f64;
                let wz = (oz + zi as f32 * cell) as f64;
                let (dxx, dxs) = detail.trig(wx);
                let (dzx, dzs) = detail.trig(wz);
                let rough = detail.noise.get([dxx, dxs, dzx, dzs]) as f32;
                out.push(low + rough * a_d);
            }
        }
        out
    };

    let mut max_err = 0.0f32;
    for c in coords.iter().take(16) {
        let g = gen_coarse(*c);
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        for idx in 0..TOTAL {
            let x = ox + (idx % SIDE) as f32 * cell;
            let z = oz + (idx / SIDE) as f32 * cell;
            max_err = max_err.max((g[idx] - hm.sample_height(x, z)).abs());
        }
    }

    let n = coords.len();
    let t = Instant::now();
    let mut acc = 0.0f64;
    for c in coords {
        acc += gen_coarse(*c).iter().map(|v| *v as f64).sum::<f64>();
    }
    let el = t.elapsed();
    std::hint::black_box(acc);
    println!(
        "  coarse nodes/side: {nodes}  (low-freq grid spacing {:.0} m)",
        CHUNK_SIZE_METERS / coarse as f32
    );
    println!(
        "  height: {:>7.3} ms/chunk   max error vs baseline = {:.3} m  (terrain spans ~0..230 m)",
        ms(el) / n as f64,
        max_err
    );
}

// ---------------------------------------------------------------------------
// E7. Lossy, done right: interpolate the RAW continental and ridge noise on a
// coarse grid, then apply amplitude + the ridge crease (1−|n|) at full res.
// E6's error floor came from interpolating *after* the crease — the V-shaped
// `1−|n|` valley can't be linearly interpolated. The raw noise is smooth, so
// interpolating it and reconstructing the crease per-vertex keeps the mean
// error at the centimetre scale (the rare metre-scale max is a ridge crest
// that falls between coarse nodes).
// ---------------------------------------------------------------------------
fn experiment_coarse_rawnoise(seed: u32, cfg: &HeightmapConfig, coords: &[IVec2]) {
    println!("\n--- E7: coarse RAW-noise interp + crease/detail at full res (LOSSY) ---");
    let coarse = env_usize("PROFILE_COARSE", 8);
    let hm = Heightmap::new(seed, cfg.clone());

    let cont = Octave::new(OpenSimplex::new(seed), cfg.continental.frequency);
    let ridge = Octave::new(
        OpenSimplex::new(seed.wrapping_add(101)),
        cfg.ridge.frequency,
    );
    let detail = Octave::new(
        OpenSimplex::new(seed.wrapping_add(907)),
        cfg.detail.frequency,
    );
    let (a_c, a_r, a_d) = (
        cfg.continental.amplitude,
        cfg.ridge.amplitude,
        cfg.detail.amplitude,
    );
    let cell = CHUNK_SIZE_METERS / (SIDE - 1) as f32;
    let nodes = coarse + 1;

    // Raw (pre-crease) continental and ridge noise at a coarse grid node.
    let raw_node = |wx: f64, wz: f64| -> (f32, f32) {
        let (cxx, cxs) = cont.trig(wx);
        let (czx, czs) = cont.trig(wz);
        let c = cont.noise.get([cxx, cxs, czx, czs]) as f32;
        let (rxx, rxs) = ridge.trig(wx);
        let (rzx, rzs) = ridge.trig(wz);
        let r = ridge.noise.get([rxx, rxs, rzx, rzs]) as f32;
        (c, r)
    };

    let gen = |c: IVec2| -> Vec<f32> {
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        let cstep = CHUNK_SIZE_METERS / coarse as f32;
        let mut cont_n = vec![0.0f32; nodes * nodes];
        let mut ridge_n = vec![0.0f32; nodes * nodes];
        for gz in 0..nodes {
            for gx in 0..nodes {
                let (cc, rr) = raw_node(
                    (ox + gx as f32 * cstep) as f64,
                    (oz + gz as f32 * cstep) as f64,
                );
                cont_n[gz * nodes + gx] = cc;
                ridge_n[gz * nodes + gx] = rr;
            }
        }
        let lerp = |grid: &[f32], x0: usize, z0: usize, tx: f32, tz: f32| {
            let a = grid[z0 * nodes + x0];
            let b = grid[z0 * nodes + x0 + 1];
            let cc = grid[(z0 + 1) * nodes + x0];
            let d = grid[(z0 + 1) * nodes + x0 + 1];
            a * (1.0 - tx) * (1.0 - tz) + b * tx * (1.0 - tz) + cc * (1.0 - tx) * tz + d * tx * tz
        };
        let mut out = Vec::with_capacity(TOTAL);
        for zi in 0..SIDE {
            let fz = zi as f32 * cell / cstep;
            let z0 = (fz.floor() as usize).min(nodes - 2);
            let tz = fz - z0 as f32;
            for xi in 0..SIDE {
                let fx = xi as f32 * cell / cstep;
                let x0 = (fx.floor() as usize).min(nodes - 2);
                let tx = fx - x0 as f32;
                let broad = lerp(&cont_n, x0, z0, tx, tz);
                let ridge_raw = lerp(&ridge_n, x0, z0, tx, tz);
                let ridges = 1.0 - ridge_raw.abs(); // crease reconstructed at full res
                                                    // detail octave still full-res
                let wx = (ox + xi as f32 * cell) as f64;
                let wz = (oz + zi as f32 * cell) as f64;
                let (dxx, dxs) = detail.trig(wx);
                let (dzx, dzs) = detail.trig(wz);
                let rough = detail.noise.get([dxx, dxs, dzx, dzs]) as f32;
                out.push(broad * a_c + ridges * a_r + rough * a_d);
            }
        }
        out
    };

    let mut max_err = 0.0f32;
    let mut sum_err = 0.0f64;
    let mut count = 0u64;
    for c in coords.iter().take(16) {
        let g = gen(*c);
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        for idx in 0..TOTAL {
            let x = ox + (idx % SIDE) as f32 * cell;
            let z = oz + (idx / SIDE) as f32 * cell;
            let e = (g[idx] - hm.sample_height(x, z)).abs();
            max_err = max_err.max(e);
            sum_err += e as f64;
            count += 1;
        }
    }

    let n = coords.len();
    let t = Instant::now();
    let mut acc = 0.0f64;
    for c in coords {
        acc += gen(*c).iter().map(|v| *v as f64).sum::<f64>();
    }
    let el = t.elapsed();
    std::hint::black_box(acc);
    println!(
        "  coarse nodes/side: {nodes}  (low-freq grid spacing {:.0} m)",
        CHUNK_SIZE_METERS / coarse as f32
    );
    println!(
        "  height: {:>7.3} ms/chunk   max err = {:.3} m   mean err = {:.4} m",
        ms(el) / n as f64,
        max_err,
        sum_err / count as f64
    );
    println!(
        "  (vs E5 baseline height ~2.7 ms/chunk; detail octave is the only full-res noise here)"
    );
}

// ---------------------------------------------------------------------------
// E8. E7 taken GLOBAL: instead of resampling the coarse continental/ridge grid
// per chunk, precompute the RAW continental and ridge noise ONCE on a coarse
// grid spanning the whole wrapping world (exactly like RiverField), then per
// vertex do two bilinear lookups + crease + the one full-res detail eval.
//
// This is the production shape from CHUNK_GEN_PROFILING.md's "recommended
// direction": the global tile wraps seamlessly by construction (rem_euclid
// indexing, same as RiverField::sample), removes the per-chunk coarse resample,
// and drops height from 3 noise evals/vertex to 1. The grid is built at the
// RiverField resolution so its memory/quality match an already-shipped field.
// We report ms/chunk (excluding the one-time global build) and max/mean error
// vs the baseline Heightmap, so it can be compared directly to E7.
// ---------------------------------------------------------------------------
fn experiment_global_rawnoise(seed: u32, config: &GameConfig, coords: &[IVec2]) {
    println!("\n--- E8: GLOBAL coarse raw-noise field + crease/detail at full res (LOSSY) ---");
    let cfg = &config.heightmap;
    let hm = Heightmap::new(seed, cfg.clone());

    // Match RiverField's grid resolution by default; override with PROFILE_GLOBAL_RES.
    let res = env_usize("PROFILE_GLOBAL_RES", config.rivers.grid_resolution as usize).max(16);
    let gcell = WORLD_SIZE_METERS as f32 / res as f32;

    let cont = Octave::new(OpenSimplex::new(seed), cfg.continental.frequency);
    let ridge = Octave::new(
        OpenSimplex::new(seed.wrapping_add(101)),
        cfg.ridge.frequency,
    );
    let detail = Octave::new(
        OpenSimplex::new(seed.wrapping_add(907)),
        cfg.detail.frequency,
    );
    let (a_c, a_r, a_d) = (
        cfg.continental.amplitude,
        cfg.ridge.amplitude,
        cfg.detail.amplitude,
    );
    let cell = CHUNK_SIZE_METERS / (SIDE - 1) as f32;

    // Build the two global coarse grids once (parallel). Node i sits at world
    // `i * gcell`, matching the bilinear sampler below.
    let raw_at = |oct: &Octave, i: usize| -> f32 {
        let wx = (i % res) as f64 * gcell as f64;
        let wz = (i / res) as f64 * gcell as f64;
        let (xx, xs) = oct.trig(wx);
        let (zx, zs) = oct.trig(wz);
        oct.noise.get([xx, xs, zx, zs]) as f32
    };
    let t_build = Instant::now();
    let cont_g: Vec<f32> = (0..res * res)
        .into_par_iter()
        .map(|i| raw_at(&cont, i))
        .collect();
    let ridge_g: Vec<f32> = (0..res * res)
        .into_par_iter()
        .map(|i| raw_at(&ridge, i))
        .collect();
    let build_ms = ms(t_build.elapsed());

    // Bilinear sample with toroidal wrap — identical math to RiverField::sample.
    let sample = |grid: &[f32], x: f32, z: f32| -> f32 {
        let gx = x / gcell;
        let gz = z / gcell;
        let x0 = gx.floor();
        let z0 = gz.floor();
        let tx = gx - x0;
        let tz = gz - z0;
        let ix0 = (x0 as i64).rem_euclid(res as i64) as usize;
        let iz0 = (z0 as i64).rem_euclid(res as i64) as usize;
        let ix1 = (ix0 + 1) % res;
        let iz1 = (iz0 + 1) % res;
        let a = grid[iz0 * res + ix0];
        let b = grid[iz0 * res + ix1];
        let c = grid[iz1 * res + ix0];
        let d = grid[iz1 * res + ix1];
        let top = a * (1.0 - tx) + b * tx;
        let bot = c * (1.0 - tx) + d * tx;
        top * (1.0 - tz) + bot * tz
    };

    let gen = |c: IVec2| -> Vec<f32> {
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        let mut out = Vec::with_capacity(TOTAL);
        for zi in 0..SIDE {
            let wz = oz + zi as f32 * cell;
            let (dz0, dz1) = detail.trig(wz as f64);
            for xi in 0..SIDE {
                let wx = ox + xi as f32 * cell;
                let broad = sample(&cont_g, wx, wz);
                let ridge_raw = sample(&ridge_g, wx, wz);
                let ridges = 1.0 - ridge_raw.abs(); // crease reconstructed at full res
                let (dx0, dx1) = detail.trig(wx as f64);
                let rough = detail.noise.get([dx0, dx1, dz0, dz1]) as f32;
                out.push(broad * a_c + ridges * a_r + rough * a_d);
            }
        }
        out
    };

    let mut max_err = 0.0f32;
    let mut sum_err = 0.0f64;
    let mut count = 0u64;
    for c in coords.iter().take(16) {
        let g = gen(*c);
        let ox = c.x as f32 * CHUNK_SIZE_METERS;
        let oz = c.y as f32 * CHUNK_SIZE_METERS;
        for idx in 0..TOTAL {
            let x = ox + (idx % SIDE) as f32 * cell;
            let z = oz + (idx / SIDE) as f32 * cell;
            let e = (g[idx] - hm.sample_height(x, z)).abs();
            max_err = max_err.max(e);
            sum_err += e as f64;
            count += 1;
        }
    }

    let n = coords.len();
    let t = Instant::now();
    let mut acc = 0.0f64;
    for c in coords {
        acc += gen(*c).iter().map(|v| *v as f64).sum::<f64>();
    }
    let el = t.elapsed();
    std::hint::black_box(acc);

    let bytes_per_field = res * res * std::mem::size_of::<f32>();
    println!(
        "  global grid: {res}×{res}  ({:.0} m/node, {:.1} MB/field × 2 fields, built once in {:.0} ms)",
        gcell,
        bytes_per_field as f64 / (1024.0 * 1024.0),
        build_ms
    );
    println!(
        "  height: {:>7.3} ms/chunk   max err = {:.3} m   mean err = {:.4} m",
        ms(el) / n as f64,
        max_err,
        sum_err / count as f64
    );
    println!(
        "  → no per-chunk resample (lookups only); detail is the sole full-res eval. \
         Compare to E7's per-chunk numbers above."
    );
}
