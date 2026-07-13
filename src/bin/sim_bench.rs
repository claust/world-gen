//! Fixed-step, headless CPU simulation benchmark.
//!
//! This binary intentionally does not create a window, renderer, GPU device, or
//! wgpu resources. It builds the same `WorldRuntime` used by the app and then
//! measures a deterministic number of `update` calls.

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use serde::Serialize;
use world_gen::world_core::config::GameConfig;
use world_gen::world_core::herbarium::{Herbarium, PlantRegistry};
use world_gen::world_core::save::{CameraSave, SaveData, WorldSave};
use world_gen::world_core::storage::{FileStorage, Storage};
use world_gen::world_runtime::{GenerationProgress, RuntimeStats, UpdateTimings, WorldRuntime};

const DEFAULT_STEPS: u32 = 60;
const DEFAULT_DT_SECONDS: f32 = 1.0 / 60.0;
const DEFAULT_DAY_SPEED: f32 = 24.0;
const DEFAULT_THREADS: usize = 1;
const DEFAULT_SEED: u32 = 42;

#[derive(Debug)]
struct Args {
    steps: u32,
    dt_seconds: f32,
    day_speed: f32,
    threads: usize,
    seed: u32,
    load_radius: i32,
    /// Start the sim clock at this `total_hours`. Base plants are born at hour
    /// 0, so a large value ages the world and makes lifecycle events due —
    /// reproducing the periodic growth-tick stall a fresh world never shows.
    start_hours: f64,
    /// Steps run (and discarded) before recording, absorbing the first tick's
    /// catch-up reap after a `--start-hours` jump.
    warmup_steps: u32,
    /// Replay a real game state: reads `save.json`, `config.json`,
    /// `herbarium.json`, `plants.bin` and `world_base.bin` from this directory
    /// (read-only). Seed, clock and camera come from the save; `--day-speed`
    /// and `--load-radius` still override when passed explicitly.
    state_dir: Option<String>,
    /// Whether `--day-speed` / `--load-radius` were passed explicitly, so state
    /// replay knows not to clobber the save/config values with defaults.
    day_speed_set: bool,
    load_radius_set: bool,
    json: bool,
}

/// Wall-clock cost of one recorded step, with the world-update phase split and
/// the per-frame HUD stats scan the app performs each frame.
#[derive(Clone, Copy, Default)]
struct StepSample {
    total_ms: f32,
    sim: UpdateTimings,
    stats_ms: f32,
}

/// One abnormally slow step, phase-attributed. Mirrors the FPS benchmark's
/// hitch report so numbers line up across the two harnesses.
#[derive(Serialize)]
struct Spike {
    step: usize,
    total_ms: f32,
    growth_ms: f32,
    spread_ms: f32,
    streaming_ms: f32,
    refresh_ms: f32,
    census_ms: f32,
    stats_ms: f32,
}

const MAX_REPORTED_SPIKES: usize = 10;

#[derive(Serialize)]
struct Report {
    benchmark: &'static str,
    seed: u32,
    steps: u32,
    dt_seconds: f32,
    simulated_hours: f64,
    day_speed: f32,
    threads: usize,
    load_radius: i32,
    setup_ms: f64,
    run_ms: f64,
    steps_per_second: f64,
    average_step_ms: f64,
    start_hours: f64,
    warmup_steps: u32,
    step_p50_ms: f32,
    step_p95_ms: f32,
    step_p99_ms: f32,
    step_max_ms: f32,
    /// Sum of each phase across all recorded steps, ms.
    growth_sum_ms: f32,
    spread_sum_ms: f32,
    streaming_sum_ms: f32,
    refresh_sum_ms: f32,
    census_sum_ms: f32,
    stats_sum_ms: f32,
    /// The slowest steps, worst first.
    spikes: Vec<Spike>,
    final_total_hours: f64,
    loaded_chunks: usize,
    pending_chunks: usize,
    world_population: usize,
    world_populated_chunks: usize,
    loaded_visible_plants: usize,
    loaded_visible_seedlings: usize,
    loaded_visible_young: usize,
    loaded_visible_mature: usize,
    loaded_visible_dead: usize,
    spread_last_added: usize,
    last_tick_ms: f32,
    resident_bytes: usize,
    checksum: u64,
}

fn main() -> anyhow::Result<()> {
    // Surface the world-load warnings (rejected base snapshot, ignored spread
    // save) that would otherwise vanish — a silently smaller world skews every
    // number this benchmark reports.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,world_gen=info"),
    )
    .init();
    let mut args = Args::parse()?;

    let setup_start = Instant::now();
    let (config, save, herbarium, spread_bytes, base_bytes) = match &args.state_dir {
        Some(dir) => {
            let storage = FileStorage::new(std::path::PathBuf::from(dir));
            let mut config = GameConfig::load(&storage);
            let mut save = SaveData::load(&storage)
                .ok_or_else(|| anyhow::anyhow!("no save.json in state dir '{dir}'"))?;
            let herbarium = Herbarium::load(&storage);
            let spread_bytes = storage.load_bytes("plants");
            let base_bytes = storage.load_bytes("world_base");
            if spread_bytes.is_none() {
                eprintln!("warning: no plants.bin in state dir '{dir}' — plant state will be the fresh base world");
            }
            // The save is the world's identity; flags only override what was
            // passed explicitly.
            args.seed = save.world.seed;
            if args.day_speed_set {
                save.world.day_speed = args.day_speed;
            } else {
                args.day_speed = save.world.day_speed;
            }
            if args.load_radius_set {
                config.world.load_radius = args.load_radius;
            } else {
                args.load_radius = config.world.load_radius;
            }
            if args.start_hours > 0.0 {
                save.world.total_hours = args.start_hours;
                save.world.hour = (args.start_hours % 24.0) as f32;
            }
            config.world.seed = save.world.seed;
            config.world.day_speed = save.world.day_speed;
            (config, save, herbarium, spread_bytes, base_bytes)
        }
        None => {
            let mut config = GameConfig::default();
            config.world.seed = args.seed;
            config.world.day_speed = args.day_speed;
            config.world.load_radius = args.load_radius;
            let total_hours = if args.start_hours > 0.0 {
                args.start_hours
            } else {
                config.world.start_hour as f64
            };
            let save = SaveData {
                camera: CameraSave {
                    position: [0.0, 96.0, 0.0],
                    yaw: 0.0,
                    pitch: 0.0,
                },
                world: WorldSave {
                    seed: args.seed,
                    hour: (total_hours % 24.0) as f32,
                    day_speed: args.day_speed,
                    total_hours,
                },
                favorites: Default::default(),
            };
            (config, save, Herbarium::default_seeded(), None, None)
        }
    };

    let gen_key = herbarium.generation_key(&config);
    let registry = Arc::new(PlantRegistry::from_herbarium(&herbarium));
    let camera = Vec3::from_array(save.camera.position);
    let progress = GenerationProgress::new();
    let mut runtime = WorldRuntime::generate(
        config,
        Some(save),
        args.threads,
        registry,
        spread_bytes,
        base_bytes,
        gen_key,
        &progress,
    )?;
    let setup_ms = setup_start.elapsed().as_secs_f64() * 1000.0;
    for _ in 0..args.warmup_steps {
        runtime.update(args.dt_seconds, camera);
        let _ = runtime.stats();
    }

    let mut samples: Vec<StepSample> = Vec::with_capacity(args.steps as usize);
    let run_start = Instant::now();
    for _ in 0..args.steps {
        let step_start = Instant::now();
        runtime.update(args.dt_seconds, camera);
        let sim = runtime.last_update_timings();
        // The app scans loaded chunks for HUD stats every frame — mirror that
        // so its cost shows up here too.
        let stats_start = Instant::now();
        let _ = runtime.stats();
        let stats_ms = stats_start.elapsed().as_secs_f32() * 1000.0;
        samples.push(StepSample {
            total_ms: step_start.elapsed().as_secs_f32() * 1000.0,
            sim,
            stats_ms,
        });
    }
    let run_ms = run_start.elapsed().as_secs_f64() * 1000.0;

    let stats = runtime.stats();
    let report = Report::new(
        &args,
        setup_ms,
        run_ms,
        runtime.total_hours(),
        stats,
        &samples,
    );

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_human(&report);
    }

    Ok(())
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut args = Self {
            steps: DEFAULT_STEPS,
            dt_seconds: DEFAULT_DT_SECONDS,
            day_speed: DEFAULT_DAY_SPEED,
            threads: DEFAULT_THREADS,
            seed: DEFAULT_SEED,
            load_radius: GameConfig::default().world.load_radius,
            start_hours: 0.0,
            warmup_steps: 0,
            state_dir: None,
            day_speed_set: false,
            load_radius_set: false,
            json: false,
        };

        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--steps" => args.steps = parse_next(&mut iter, "--steps")?,
                "--dt" => args.dt_seconds = parse_next(&mut iter, "--dt")?,
                "--day-speed" => {
                    args.day_speed = parse_next(&mut iter, "--day-speed")?;
                    args.day_speed_set = true;
                }
                "--threads" => args.threads = parse_next(&mut iter, "--threads")?,
                "--seed" => args.seed = parse_next(&mut iter, "--seed")?,
                "--load-radius" => {
                    args.load_radius = parse_next(&mut iter, "--load-radius")?;
                    args.load_radius_set = true;
                }
                "--start-hours" => args.start_hours = parse_next(&mut iter, "--start-hours")?,
                "--warmup-steps" => args.warmup_steps = parse_next(&mut iter, "--warmup-steps")?,
                "--state-dir" => args.state_dir = Some(parse_next(&mut iter, "--state-dir")?),
                "--json" => args.json = true,
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => anyhow::bail!("unknown argument '{other}'"),
            }
        }

        if args.steps == 0 {
            anyhow::bail!("--steps must be greater than 0");
        }
        if args.dt_seconds <= 0.0 || !args.dt_seconds.is_finite() {
            anyhow::bail!("--dt must be a finite positive number");
        }
        if args.day_speed < 0.0 || !args.day_speed.is_finite() {
            anyhow::bail!("--day-speed must be a finite non-negative number");
        }
        if args.threads == 0 {
            anyhow::bail!("--threads must be greater than 0");
        }
        if args.load_radius < 0 {
            anyhow::bail!("--load-radius must be non-negative");
        }
        if args.start_hours < 0.0 || !args.start_hours.is_finite() {
            anyhow::bail!("--start-hours must be a finite non-negative number");
        }

        Ok(args)
    }
}

impl Report {
    fn new(
        args: &Args,
        setup_ms: f64,
        run_ms: f64,
        final_total_hours: f64,
        stats: RuntimeStats,
        samples: &[StepSample],
    ) -> Self {
        let simulated_hours = args.steps as f64 * args.dt_seconds as f64 * args.day_speed as f64;
        let run_seconds = (run_ms / 1000.0).max(f64::EPSILON);
        let steps_per_second = args.steps as f64 / run_seconds;
        let average_step_ms = run_ms / args.steps as f64;
        let checksum = checksum(args, final_total_hours, &stats);

        let mut sorted: Vec<f32> = samples.iter().map(|s| s.total_ms).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let pct = |p: f64| -> f32 {
            if sorted.is_empty() {
                return 0.0;
            }
            let idx = ((p / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
            sorted[idx.min(sorted.len() - 1)]
        };

        let sum = |f: &dyn Fn(&StepSample) -> f32| samples.iter().map(f).sum::<f32>();

        let mut spikes: Vec<Spike> = samples
            .iter()
            .enumerate()
            .map(|(step, s)| Spike {
                step,
                total_ms: s.total_ms,
                growth_ms: s.sim.growth_ms,
                spread_ms: s.sim.spread_ms,
                streaming_ms: s.sim.streaming_ms,
                refresh_ms: s.sim.refresh_ms,
                census_ms: s.sim.census_ms,
                stats_ms: s.stats_ms,
            })
            .collect();
        spikes.sort_by(|a, b| {
            b.total_ms
                .partial_cmp(&a.total_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        spikes.truncate(MAX_REPORTED_SPIKES);

        Self {
            benchmark: "fixed-step-simulation",
            seed: args.seed,
            steps: args.steps,
            dt_seconds: args.dt_seconds,
            simulated_hours,
            day_speed: args.day_speed,
            threads: args.threads,
            load_radius: args.load_radius,
            setup_ms,
            run_ms,
            steps_per_second,
            average_step_ms,
            start_hours: args.start_hours,
            warmup_steps: args.warmup_steps,
            step_p50_ms: pct(50.0),
            step_p95_ms: pct(95.0),
            step_p99_ms: pct(99.0),
            step_max_ms: sorted.last().copied().unwrap_or(0.0),
            growth_sum_ms: sum(&|s| s.sim.growth_ms),
            spread_sum_ms: sum(&|s| s.sim.spread_ms),
            streaming_sum_ms: sum(&|s| s.sim.streaming_ms),
            refresh_sum_ms: sum(&|s| s.sim.refresh_ms),
            census_sum_ms: sum(&|s| s.sim.census_ms),
            stats_sum_ms: sum(&|s| s.stats_ms),
            spikes,
            final_total_hours,
            loaded_chunks: stats.loaded_chunks,
            pending_chunks: stats.pending_chunks,
            world_population: stats.world_population,
            world_populated_chunks: stats.world_populated_chunks,
            loaded_visible_plants: stats.loaded_visible_plants,
            loaded_visible_seedlings: stats.loaded_visible_seedlings,
            loaded_visible_young: stats.loaded_visible_young,
            loaded_visible_mature: stats.loaded_visible_mature,
            loaded_visible_dead: stats.loaded_visible_dead,
            spread_last_added: stats.spread_last_added,
            last_tick_ms: stats.tick_ms,
            resident_bytes: stats.resident_bytes,
            checksum,
        }
    }
}

fn parse_next<T: std::str::FromStr>(
    iter: &mut impl Iterator<Item = String>,
    flag: &str,
) -> anyhow::Result<T>
where
    T::Err: std::fmt::Display,
{
    let raw = iter
        .next()
        .filter(|value| !value.starts_with("--"))
        .ok_or_else(|| anyhow::anyhow!("{flag} requires a value"))?;
    raw.parse::<T>()
        .map_err(|err| anyhow::anyhow!("invalid {flag} value '{raw}': {err}"))
}

fn checksum(args: &Args, final_total_hours: f64, stats: &RuntimeStats) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for value in [
        args.seed as u64,
        args.steps as u64,
        args.dt_seconds.to_bits() as u64,
        args.day_speed.to_bits() as u64,
        final_total_hours.to_bits(),
        stats.loaded_chunks as u64,
        stats.pending_chunks as u64,
        stats.world_population as u64,
        stats.world_populated_chunks as u64,
        stats.loaded_visible_plants as u64,
        stats.loaded_visible_seedlings as u64,
        stats.loaded_visible_young as u64,
        stats.loaded_visible_mature as u64,
        stats.loaded_visible_dead as u64,
        stats.spread_last_added as u64,
        stats.resident_bytes as u64,
    ] {
        hash ^= value;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn print_human(report: &Report) {
    println!("fixed-step simulation benchmark");
    println!("seed              : {}", report.seed);
    println!("steps             : {}", report.steps);
    println!("dt                : {:.6}s", report.dt_seconds);
    println!("day speed         : {:.3} sim-hours/sec", report.day_speed);
    println!("simulated time    : {:.3} hours", report.simulated_hours);
    println!("threads           : {}", report.threads);
    println!("load radius       : {}", report.load_radius);
    println!("setup             : {:.2} ms", report.setup_ms);
    println!("run               : {:.2} ms", report.run_ms);
    println!("steps/sec         : {:.2}", report.steps_per_second);
    println!("avg step          : {:.4} ms", report.average_step_ms);
    println!("start hours       : {:.1}", report.start_hours);
    println!("warmup steps      : {}", report.warmup_steps);
    println!(
        "step p50/p95/p99  : {:.3} / {:.3} / {:.3} ms (max {:.1})",
        report.step_p50_ms, report.step_p95_ms, report.step_p99_ms, report.step_max_ms
    );
    println!(
        "phase sums        : growth {:.1} | spread {:.1} | streaming {:.1} | refresh {:.1} | census {:.1} | stats {:.1} ms",
        report.growth_sum_ms,
        report.spread_sum_ms,
        report.streaming_sum_ms,
        report.refresh_sum_ms,
        report.census_sum_ms,
        report.stats_sum_ms,
    );
    if !report.spikes.is_empty() && report.step_max_ms > 1.0 {
        println!(
            "{:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
            "step", "total_ms", "growth", "spread", "stream", "refresh", "census", "stats"
        );
        for s in &report.spikes {
            println!(
                "{:>7} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1}",
                s.step,
                s.total_ms,
                s.growth_ms,
                s.spread_ms,
                s.streaming_ms,
                s.refresh_ms,
                s.census_ms,
                s.stats_ms,
            );
        }
    }
    println!("population        : {}", report.world_population);
    println!("populated chunks  : {}", report.world_populated_chunks);
    println!("loaded chunks     : {}", report.loaded_chunks);
    println!("pending chunks    : {}", report.pending_chunks);
    println!("visible plants    : {}", report.loaded_visible_plants);
    println!("spread last added : {}", report.spread_last_added);
    println!("last tick         : {:.4} ms", report.last_tick_ms);
    println!("resident bytes    : {}", report.resident_bytes);
    println!("checksum          : {:#018x}", report.checksum);
}

fn print_usage() {
    println!(
        "\
Usage: cargo run --release --bin sim_bench -- [options]

Options:
  --steps <n>          Fixed number of WorldRuntime::update calls (default {DEFAULT_STEPS})
  --dt <seconds>       Fixed timestep in seconds (default {DEFAULT_DT_SECONDS})
  --day-speed <hours>  Sim-hours advanced per real second (default {DEFAULT_DAY_SPEED})
  --threads <n>        Worker threads for generation/streaming (default {DEFAULT_THREADS})
  --seed <n>           World seed (default {DEFAULT_SEED})
  --load-radius <n>    CPU streaming radius (default from GameConfig)
  --start-hours <h>    Start the sim clock at this total_hours; ages the world so
                       lifecycle events are due during the run (default 0 = fresh)
  --warmup-steps <n>   Steps run before recording, absorbing the first tick's
                       catch-up reap after a --start-hours jump (default 0)
  --state-dir <dir>    Replay a real game state directory (save.json, config.json,
                       plants.bin, world_base.bin; read-only). Seed/clock/camera
                       come from the save; explicit flags still override
  --json               Emit JSON instead of human-readable text
  --help               Show this help
"
    );
}
