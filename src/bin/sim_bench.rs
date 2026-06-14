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
use world_gen::world_runtime::{GenerationProgress, RuntimeStats, WorldRuntime};

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
    json: bool,
}

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
    let args = Args::parse()?;

    let setup_start = Instant::now();
    let mut config = GameConfig::default();
    config.world.seed = args.seed;
    config.world.day_speed = args.day_speed;
    config.world.load_radius = args.load_radius;

    let herbarium = Herbarium::default_seeded();
    let gen_key = herbarium.generation_key(&config);
    let registry = Arc::new(PlantRegistry::from_herbarium(&herbarium));
    let save = SaveData {
        camera: CameraSave {
            position: [0.0, 96.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
        },
        world: WorldSave {
            seed: args.seed,
            hour: config.world.start_hour,
            day_speed: args.day_speed,
            total_hours: config.world.start_hour as f64,
        },
        favorites: Default::default(),
    };
    let progress = GenerationProgress::new();
    let mut runtime = WorldRuntime::generate(
        config,
        Some(save),
        args.threads,
        registry,
        None,
        None,
        gen_key,
        &progress,
    )?;
    let setup_ms = setup_start.elapsed().as_secs_f64() * 1000.0;

    let camera = Vec3::new(0.0, 96.0, 0.0);
    let run_start = Instant::now();
    for _ in 0..args.steps {
        runtime.update(args.dt_seconds, camera);
    }
    let run_ms = run_start.elapsed().as_secs_f64() * 1000.0;

    let stats = runtime.stats();
    let report = Report::new(&args, setup_ms, run_ms, runtime.total_hours(), stats);

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
            json: false,
        };

        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--steps" => args.steps = parse_next(&mut iter, "--steps")?,
                "--dt" => args.dt_seconds = parse_next(&mut iter, "--dt")?,
                "--day-speed" => args.day_speed = parse_next(&mut iter, "--day-speed")?,
                "--threads" => args.threads = parse_next(&mut iter, "--threads")?,
                "--seed" => args.seed = parse_next(&mut iter, "--seed")?,
                "--load-radius" => args.load_radius = parse_next(&mut iter, "--load-radius")?,
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
    ) -> Self {
        let simulated_hours = args.steps as f64 * args.dt_seconds as f64 * args.day_speed as f64;
        let steps_per_second = args.steps as f64 / (run_ms / 1000.0);
        let average_step_ms = run_ms / args.steps as f64;
        let checksum = checksum(args, final_total_hours, &stats);

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
  --json               Emit JSON instead of human-readable text
  --help               Show this help
"
    );
}
