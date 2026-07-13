//! Deterministic FPS benchmark mode.
//!
//! Entered with `--benchmark [path]` (see `main.rs`). Instead of reading live
//! input, the camera is driven through a scripted flythrough loaded from JSON.
//! Movement is advanced on a **fixed timestep** so the simulated workload (camera
//! path, which chunks stream in, what is on screen) is identical on every run —
//! only the measured wall-clock frame time varies. When the script finishes a
//! JSON report is written next to the script (`latest.json`) and the app exits.
//!
//! See `tools/bench.ts` for the one-command wrapper that builds, runs, and prints
//! the report.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::renderer_wgpu::camera::{CameraController, FlyCamera, MoveDirection};
use crate::world_runtime::UpdateTimings;

/// All movement directions, used to clear/set the synthetic key state each frame.
const ALL_DIRECTIONS: [MoveDirection; 6] = [
    MoveDirection::Forward,
    MoveDirection::Backward,
    MoveDirection::Left,
    MoveDirection::Right,
    MoveDirection::Up,
    MoveDirection::Down,
];

fn parse_direction(key: &str) -> Option<MoveDirection> {
    match key.to_ascii_lowercase().as_str() {
        "w" | "forward" => Some(MoveDirection::Forward),
        "s" | "backward" | "back" => Some(MoveDirection::Backward),
        "a" | "left" => Some(MoveDirection::Left),
        "d" | "right" => Some(MoveDirection::Right),
        "up" | "space" => Some(MoveDirection::Up),
        "down" | "shift" => Some(MoveDirection::Down),
        _ => None,
    }
}

// --- Script (input) ---------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
struct StartPose {
    x: f32,
    y: f32,
    z: f32,
    #[serde(default)]
    yaw: f32,
    #[serde(default)]
    pitch: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct Segment {
    /// Movement keys held down for the whole segment (e.g. ["w", "d"]).
    #[serde(default)]
    keys: Vec<String>,
    /// Yaw turn rate in radians/second (positive = turn right).
    #[serde(default)]
    yaw_rate: f32,
    /// Pitch turn rate in radians/second (positive = look up).
    #[serde(default)]
    pitch_rate: f32,
    /// Number of frames this segment lasts.
    frames: u32,
}

fn default_fixed_dt() -> f32 {
    1.0 / 60.0
}

fn default_warmup() -> u32 {
    90
}

#[derive(Debug, Clone, Deserialize)]
struct BenchmarkScript {
    #[serde(default = "default_name")]
    name: String,
    /// Simulation timestep used for every frame, in seconds. Decoupling movement
    /// from real frame time is what makes runs comparable.
    #[serde(default = "default_fixed_dt")]
    fixed_dt: f32,
    /// Frames rendered (stationary) before recording starts — absorbs shader
    /// compilation and initial chunk-streaming spikes.
    #[serde(default = "default_warmup")]
    warmup_frames: u32,
    /// Override the world clock speed (sim-hours per real second) for the run.
    /// Benchmark mode always starts a fresh world, whose config default may make
    /// plant-sim ticks too rare to observe; with the fixed timestep this stays
    /// deterministic. `None` keeps the config value.
    #[serde(default)]
    day_speed: Option<f32>,
    /// Jump the sim clock to this `total_hours` once the world is loaded. Base
    /// plants are born at hour 0, so a large value ages the whole world and makes
    /// lifecycle events (stage changes, death, despawn) due during the run —
    /// a fresh world's first days have none, hiding plant-sim hitches.
    /// Make `warmup_frames` cover at least one growth tick (one sim-hour) so the
    /// first tick's catch-up reap of everything that died in the skipped time
    /// isn't recorded as a steady-state frame.
    #[serde(default)]
    start_total_hours: Option<f64>,
    /// Resume the existing save instead of starting a fresh world. A long-played
    /// save carries a plant population orders of magnitude beyond anything a
    /// fresh world reaches, which is where plant-sim stalls live. Not
    /// reproducible across machines — for diagnosis, not CI baselines. Saving is
    /// disabled throughout benchmark mode, so the replayed state stays untouched.
    #[serde(default)]
    resume: bool,
    start: StartPose,
    segments: Vec<Segment>,
}

/// One-shot world-state overrides the app applies when the benchmark starts.
#[derive(Debug, Clone, Copy)]
pub struct WorldOverrides {
    pub day_speed: Option<f32>,
    pub start_total_hours: Option<f64>,
}

fn default_name() -> String {
    "flythrough".to_string()
}

// --- Report (output) --------------------------------------------------------

/// CPU-vs-GPU bound breakdown, derived from CPU-side timers (works on every
/// backend, including Apple Metal where GPU timestamp queries do not bracket
/// fragment work). `null` in the JSON when no samples were collected.
///
/// Per frame we measure:
/// - `gpu_wait_ms`: time the CPU blocks acquiring the next swapchain image, i.e.
///   waiting for the GPU to finish earlier frames. When GPU-bound this closely
///   tracks the GPU's per-frame time.
/// - `cpu_ms`: CPU-active work for the frame (simulation/streaming in `update`
///   plus command encoding in `render`), excluding the GPU wait.
#[derive(Debug, Clone, Serialize)]
pub struct BoundTimings {
    pub samples: usize,
    pub mean_gpu_wait_ms: f64,
    pub p95_gpu_wait_ms: f64,
    pub mean_cpu_ms: f64,
    pub p95_cpu_ms: f64,
    /// `mean_gpu_wait / (mean_gpu_wait + mean_cpu)`. ~1.0 ⇒ GPU-bound (CPU idle,
    /// waiting on the GPU); near 0 ⇒ CPU-bound.
    pub gpu_bound_ratio: f64,
}

/// Aggregate cost of one update phase across the recorded frames.
#[derive(Debug, Clone, Serialize)]
pub struct PhaseEntry {
    pub name: &'static str,
    pub max_ms: f64,
    /// Total across the whole recording — what the phase cost the run.
    pub sum_ms: f64,
    /// Frames where the phase actually ran (cost above the timer-noise floor).
    pub active_frames: usize,
}

/// One abnormally slow frame, with the per-phase sim costs measured that frame
/// so the stall can be attributed.
#[derive(Debug, Clone, Serialize)]
pub struct Hitch {
    /// Index into the recorded (post-warmup) frames.
    pub frame: usize,
    pub cpu_ms: f32,
    pub growth_ms: f32,
    pub spread_ms: f32,
    pub streaming_ms: f32,
    pub refresh_ms: f32,
    pub census_ms: f32,
    pub hud_ms: f32,
}

/// A frame whose CPU-active time is at least this is reported as a hitch.
const HITCH_THRESHOLD_MS: f32 = 50.0;

/// How many of the slowest hitches the report lists individually.
const MAX_REPORTED_HITCHES: usize = 10;

/// Phase costs below this (ms) count as "didn't run" for `active_frames` —
/// they're timer overhead, not work.
const PHASE_NOISE_FLOOR_MS: f32 = 0.05;

#[derive(Debug, Clone, Serialize)]
pub struct BenchmarkReport {
    pub name: String,
    pub fixed_dt: f32,
    pub warmup_frames: u32,
    pub frames: usize,
    pub avg_fps: f64,
    pub min_fps: f64,
    pub max_fps: f64,
    pub one_percent_low_fps: f64,
    pub mean_frame_ms: f64,
    pub p50_frame_ms: f64,
    pub p95_frame_ms: f64,
    pub p99_frame_ms: f64,
    /// CPU-vs-GPU bound breakdown, or `null` when no samples were collected.
    pub bound: Option<BoundTimings>,
    /// Recorded frames whose CPU time crossed [`HITCH_THRESHOLD_MS`].
    pub hitch_count: usize,
    pub hitch_threshold_ms: f32,
    /// The slowest hitches, worst first, with per-phase attribution.
    pub worst_hitches: Vec<Hitch>,
    /// Per-phase aggregate costs of the world update across recorded frames.
    pub phases: Vec<PhaseEntry>,
}

/// Mean and p50/p95/p99 percentiles of a millisecond sample set. Returns zeros
/// for an empty set. `percentile` indexes into the ascending-sorted samples.
fn ms_stats(samples: &[f32]) -> (f64, f64, f64, f64) {
    if samples.is_empty() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let mut sorted: Vec<f64> = samples.iter().map(|&ms| ms as f64).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let mean = sorted.iter().sum::<f64>() / n as f64;
    let percentile = |p: f64| -> f64 {
        let idx = ((p / 100.0) * (n as f64 - 1.0)).round() as usize;
        sorted[idx.min(n - 1)]
    };
    (mean, percentile(50.0), percentile(95.0), percentile(99.0))
}

/// Per-phase aggregates and hitch attribution from the aligned per-frame
/// recordings (`cpu_samples[i]`, `sim_samples[i]`, `hud_samples[i]` are the same
/// frame).
fn analyze_hitches(
    cpu_samples: &[f32],
    sim_samples: &[UpdateTimings],
    hud_samples: &[f32],
) -> (usize, Vec<Hitch>, Vec<PhaseEntry>) {
    let sim = |i: usize| sim_samples.get(i).copied().unwrap_or_default();
    let hud = |i: usize| hud_samples.get(i).copied().unwrap_or(0.0);

    let mut hitches: Vec<Hitch> = cpu_samples
        .iter()
        .enumerate()
        .filter(|&(_, &cpu_ms)| cpu_ms >= HITCH_THRESHOLD_MS)
        .map(|(i, &cpu_ms)| {
            let t = sim(i);
            Hitch {
                frame: i,
                cpu_ms,
                growth_ms: t.growth_ms,
                spread_ms: t.spread_ms,
                streaming_ms: t.streaming_ms,
                refresh_ms: t.refresh_ms,
                census_ms: t.census_ms,
                hud_ms: hud(i),
            }
        })
        .collect();
    let hitch_count = hitches.len();
    hitches.sort_by(|a, b| {
        b.cpu_ms
            .partial_cmp(&a.cpu_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hitches.truncate(MAX_REPORTED_HITCHES);

    let phase = |name: &'static str, extract: &dyn Fn(usize) -> f32| {
        let mut max_ms = 0.0f64;
        let mut sum_ms = 0.0f64;
        let mut active_frames = 0usize;
        for i in 0..cpu_samples.len() {
            let v = extract(i);
            max_ms = max_ms.max(v as f64);
            sum_ms += v as f64;
            if v > PHASE_NOISE_FLOOR_MS {
                active_frames += 1;
            }
        }
        PhaseEntry {
            name,
            max_ms,
            sum_ms,
            active_frames,
        }
    };
    let phases = vec![
        phase("growth", &|i| sim(i).growth_ms),
        phase("spread", &|i| sim(i).spread_ms),
        phase("streaming", &|i| sim(i).streaming_ms),
        phase("refresh", &|i| sim(i).refresh_ms),
        phase("census", &|i| sim(i).census_ms),
        phase("hud", &hud),
    ];

    (hitch_count, hitches, phases)
}

/// Computes summary statistics from per-frame CPU durations (milliseconds) and,
/// when available, the CPU-vs-GPU bound breakdown.
#[allow(clippy::too_many_arguments)]
fn summarize(
    name: &str,
    fixed_dt: f32,
    warmup_frames: u32,
    samples: &[f32],
    gpu_wait_samples: &[f32],
    cpu_samples: &[f32],
    sim_samples: &[UpdateTimings],
    hud_samples: &[f32],
) -> BenchmarkReport {
    let (hitch_count, worst_hitches, phases) =
        analyze_hitches(cpu_samples, sim_samples, hud_samples);
    if samples.is_empty() {
        return BenchmarkReport {
            name: name.to_string(),
            fixed_dt,
            warmup_frames,
            frames: 0,
            avg_fps: 0.0,
            min_fps: 0.0,
            max_fps: 0.0,
            one_percent_low_fps: 0.0,
            mean_frame_ms: 0.0,
            p50_frame_ms: 0.0,
            p95_frame_ms: 0.0,
            p99_frame_ms: 0.0,
            bound: None,
            hitch_count,
            hitch_threshold_ms: HITCH_THRESHOLD_MS,
            worst_hitches,
            phases,
        };
    }

    let mut sorted: Vec<f64> = samples.iter().map(|&ms| ms as f64).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let n = sorted.len();
    let sum: f64 = sorted.iter().sum();
    let mean_ms = sum / n as f64;

    // Percentile of frame time: index into the ascending-sorted samples.
    let percentile = |p: f64| -> f64 {
        let idx = ((p / 100.0) * (n as f64 - 1.0)).round() as usize;
        sorted[idx.min(n - 1)]
    };

    // 1% low FPS: average of the slowest 1% of frames (largest frame times).
    let low_count = (n / 100).max(1);
    let slow_sum: f64 = sorted[n - low_count..].iter().sum();
    let slow_mean = slow_sum / low_count as f64;

    let fps = |ms: f64| if ms > 0.0 { 1000.0 / ms } else { 0.0 };

    // Bound breakdown is optional: present only when frame timings were recorded.
    let bound = if gpu_wait_samples.is_empty() {
        None
    } else {
        let (gpu_wait_mean, _, gpu_wait_p95, _) = ms_stats(gpu_wait_samples);
        let (cpu_mean, _, cpu_p95, _) = ms_stats(cpu_samples);
        let denom = gpu_wait_mean + cpu_mean;
        Some(BoundTimings {
            samples: gpu_wait_samples.len(),
            mean_gpu_wait_ms: gpu_wait_mean,
            p95_gpu_wait_ms: gpu_wait_p95,
            mean_cpu_ms: cpu_mean,
            p95_cpu_ms: cpu_p95,
            gpu_bound_ratio: if denom > 0.0 {
                gpu_wait_mean / denom
            } else {
                0.0
            },
        })
    };

    BenchmarkReport {
        name: name.to_string(),
        fixed_dt,
        warmup_frames,
        frames: n,
        avg_fps: fps(mean_ms),
        min_fps: fps(sorted[n - 1]),
        max_fps: fps(sorted[0]),
        one_percent_low_fps: fps(slow_mean),
        mean_frame_ms: mean_ms,
        p50_frame_ms: percentile(50.0),
        p95_frame_ms: percentile(95.0),
        p99_frame_ms: percentile(99.0),
        bound,
        hitch_count,
        hitch_threshold_ms: HITCH_THRESHOLD_MS,
        worst_hitches,
        phases,
    }
}

// --- Runtime state machine --------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the world to finish loading and the first Playing frame.
    WaitWorld,
    /// Rendering `remaining` stationary frames before recording starts.
    Warmup {
        remaining: u32,
    },
    /// Replaying `segment` with `remaining` frames left in it.
    Running {
        segment: usize,
        remaining: u32,
    },
    Done,
}

pub struct Benchmark {
    script: BenchmarkScript,
    phase: Phase,
    samples: Vec<f32>,
    /// Per-frame GPU-wait (swapchain acquire stall) durations, ms, while Running.
    gpu_wait_samples: Vec<f32>,
    /// Per-frame CPU-active durations, ms, while Running.
    cpu_samples: Vec<f32>,
    /// Per-frame world-update phase breakdown, aligned with `cpu_samples`.
    sim_samples: Vec<UpdateTimings>,
    /// Per-frame HUD stats+upload cost (ms), aligned with `cpu_samples`.
    hud_samples: Vec<f32>,
    /// Whether the script's world overrides have been handed to the app yet.
    world_overrides_taken: bool,
    output_path: PathBuf,
}

impl Benchmark {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read benchmark script {}", path.display()))?;
        let script: BenchmarkScript = serde_json::from_str(&text)
            .with_context(|| format!("failed to parse benchmark script {}", path.display()))?;

        let output_path = path
            .parent()
            .map(|p| p.join("latest.json"))
            .unwrap_or_else(|| PathBuf::from("benchmarks/latest.json"));

        log::info!(
            "benchmark mode: '{}' — {} segments, fixed_dt={:.5}s, warmup={} frames",
            script.name,
            script.segments.len(),
            script.fixed_dt,
            script.warmup_frames
        );

        Ok(Self {
            script,
            phase: Phase::WaitWorld,
            samples: Vec::new(),
            gpu_wait_samples: Vec::new(),
            cpu_samples: Vec::new(),
            sim_samples: Vec::new(),
            hud_samples: Vec::new(),
            world_overrides_taken: false,
            output_path,
        })
    }

    pub fn fixed_dt(&self) -> f32 {
        self.script.fixed_dt
    }

    /// Whether the script asks to resume the existing save instead of starting
    /// a fresh world.
    pub fn resume(&self) -> bool {
        self.script.resume
    }

    /// The script's one-shot world-state overrides, or `None` after the first
    /// call (or when the script sets none). The app applies them to the live
    /// world on the first Playing frame.
    pub fn take_world_overrides(&mut self) -> Option<WorldOverrides> {
        if self.world_overrides_taken {
            return None;
        }
        self.world_overrides_taken = true;
        if self.script.day_speed.is_none() && self.script.start_total_hours.is_none() {
            return None;
        }
        Some(WorldOverrides {
            day_speed: self.script.day_speed,
            start_total_hours: self.script.start_total_hours,
        })
    }

    pub fn is_finished(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Records a frame's GPU-wait (swapchain acquire stall) and CPU-active
    /// durations (ms), plus the frame's world-update phase breakdown and HUD
    /// cost for hitch attribution. Only retained while actively recording, so
    /// warmup/load-time work is excluded — matching the CPU frame samples.
    pub fn record_frame_timing(
        &mut self,
        gpu_wait_ms: f32,
        cpu_ms: f32,
        sim: UpdateTimings,
        hud_ms: f32,
    ) {
        if matches!(self.phase, Phase::Running { .. }) {
            self.gpu_wait_samples.push(gpu_wait_ms);
            self.cpu_samples.push(cpu_ms);
            self.sim_samples.push(sim);
            self.hud_samples.push(hud_ms);
        }
    }

    /// Picks the phase that follows warmup: the first non-empty segment, or Done.
    fn first_running_phase(&self) -> Phase {
        for (i, seg) in self.script.segments.iter().enumerate() {
            if seg.frames > 0 {
                return Phase::Running {
                    segment: i,
                    remaining: seg.frames,
                };
            }
        }
        Phase::Done
    }

    /// Advances the next segment with frames > 0 after `from`, or Done.
    fn advance_segment(&self, from: usize) -> Phase {
        let mut i = from + 1;
        while i < self.script.segments.len() {
            if self.script.segments[i].frames > 0 {
                return Phase::Running {
                    segment: i,
                    remaining: self.script.segments[i].frames,
                };
            }
            i += 1;
        }
        Phase::Done
    }

    fn apply_segment_input(
        &self,
        segment: usize,
        camera: &mut FlyCamera,
        controller: &mut CameraController,
    ) {
        let seg = &self.script.segments[segment];
        let active: Vec<MoveDirection> =
            seg.keys.iter().filter_map(|k| parse_direction(k)).collect();
        for dir in ALL_DIRECTIONS {
            let pressed = active
                .iter()
                .any(|d| std::mem::discriminant(d) == std::mem::discriminant(&dir));
            controller.set_remote_move(dir, pressed);
        }
        let dt = self.script.fixed_dt;
        camera.yaw += seg.yaw_rate * dt;
        camera.pitch = (camera.pitch + seg.pitch_rate * dt).clamp(-1.54, 1.54);
    }

    fn clear_input(&self, controller: &mut CameraController) {
        for dir in ALL_DIRECTIONS {
            controller.set_remote_move(dir, false);
        }
    }

    /// Called once per frame while the world is Playing. `real_frame_ms` is the
    /// actual wall-clock duration of the previous frame (the thing we measure).
    pub fn tick(
        &mut self,
        real_frame_ms: f32,
        camera: &mut FlyCamera,
        controller: &mut CameraController,
    ) {
        // Once finished, do nothing — the report has already been written.
        if self.phase == Phase::Done {
            return;
        }

        match self.phase {
            Phase::WaitWorld => {
                // First Playing frame: snap to the start pose and clear input.
                let start = &self.script.start;
                camera.position = glam::Vec3::new(start.x, start.y, start.z);
                camera.yaw = start.yaw;
                camera.pitch = start.pitch.clamp(-1.54, 1.54);
                self.clear_input(controller);

                self.phase = if self.script.warmup_frames > 0 {
                    Phase::Warmup {
                        remaining: self.script.warmup_frames,
                    }
                } else {
                    self.first_running_phase()
                };
            }
            Phase::Warmup { remaining } => {
                self.clear_input(controller);
                let left = remaining - 1;
                self.phase = if left == 0 {
                    self.first_running_phase()
                } else {
                    Phase::Warmup { remaining: left }
                };
            }
            Phase::Running { segment, remaining } => {
                self.apply_segment_input(segment, camera, controller);
                self.samples.push(real_frame_ms);

                let left = remaining - 1;
                if left == 0 {
                    self.phase = self.advance_segment(segment);
                } else {
                    self.phase = Phase::Running {
                        segment,
                        remaining: left,
                    };
                }
            }
            Phase::Done => {}
        }

        if self.phase == Phase::Done {
            self.clear_input(controller);
            self.finish();
        }
    }

    /// Computes the report, writes it to disk, and logs a human-readable summary.
    fn finish(&mut self) {
        let report = summarize(
            &self.script.name,
            self.script.fixed_dt,
            self.script.warmup_frames,
            &self.samples,
            &self.gpu_wait_samples,
            &self.cpu_samples,
            &self.sim_samples,
            &self.hud_samples,
        );

        if let Some(parent) = self.output_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.output_path, format!("{json}\n")) {
                    log::error!("failed to write benchmark report: {e}");
                } else {
                    log::info!("benchmark report written to {}", self.output_path.display());
                }
            }
            Err(e) => log::error!("failed to serialize benchmark report: {e}"),
        }

        // Also print to stdout so the wrapper / terminal shows it directly.
        println!(
            "\n=== BENCHMARK '{}' ===\n\
             frames recorded : {}\n\
             average FPS      : {:.1}\n\
             1% low FPS       : {:.1}\n\
             min / max FPS    : {:.1} / {:.1}\n\
             mean frame time  : {:.3} ms\n\
             p50 / p95 / p99  : {:.3} / {:.3} / {:.3} ms",
            report.name,
            report.frames,
            report.avg_fps,
            report.one_percent_low_fps,
            report.min_fps,
            report.max_fps,
            report.mean_frame_ms,
            report.p50_frame_ms,
            report.p95_frame_ms,
            report.p99_frame_ms,
        );
        match &report.bound {
            Some(b) => println!(
                "GPU wait (mean)  : {:.3} ms (p95 {:.3})\n\
                 CPU work (mean)  : {:.3} ms (p95 {:.3})\n\
                 GPU-bound ratio  : {:.3}",
                b.mean_gpu_wait_ms,
                b.p95_gpu_wait_ms,
                b.mean_cpu_ms,
                b.p95_cpu_ms,
                b.gpu_bound_ratio,
            ),
            None => println!("bound breakdown  : unavailable (no samples)"),
        }

        println!(
            "hitches (>{:.0}ms) : {}",
            report.hitch_threshold_ms, report.hitch_count
        );
        if !report.worst_hitches.is_empty() {
            println!(
                "{:>7} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}",
                "frame", "cpu_ms", "growth", "spread", "stream", "refresh", "census", "hud"
            );
            for h in &report.worst_hitches {
                println!(
                    "{:>7} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1} {:>9.1}",
                    h.frame,
                    h.cpu_ms,
                    h.growth_ms,
                    h.spread_ms,
                    h.streaming_ms,
                    h.refresh_ms,
                    h.census_ms,
                    h.hud_ms,
                );
            }
        }
        println!("phase totals over the run (ms):");
        for p in &report.phases {
            println!(
                "  {:<9} max {:>8.1}  sum {:>9.1}  active frames {:>5}",
                p.name, p.max_ms, p.sum_ms, p.active_frames
            );
        }
        println!("======================\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direction_aliases() {
        assert!(matches!(parse_direction("w"), Some(MoveDirection::Forward)));
        assert!(matches!(parse_direction("UP"), Some(MoveDirection::Up)));
        assert!(matches!(parse_direction("left"), Some(MoveDirection::Left)));
        assert!(parse_direction("q").is_none());
    }

    #[test]
    fn summarize_empty_is_zeroed() {
        let r = summarize("x", 0.016, 10, &[], &[], &[], &[], &[]);
        assert_eq!(r.frames, 0);
        assert_eq!(r.avg_fps, 0.0);
        assert!(r.bound.is_none());
    }

    #[test]
    fn summarize_constant_16ms_is_about_60fps() {
        let samples = vec![16.0_f32; 200];
        let r = summarize("x", 0.016, 0, &samples, &[], &[], &[], &[]);
        assert_eq!(r.frames, 200);
        assert!((r.avg_fps - 62.5).abs() < 0.01);
        assert!((r.min_fps - 62.5).abs() < 0.01);
        assert!((r.max_fps - 62.5).abs() < 0.01);
        assert!((r.one_percent_low_fps - 62.5).abs() < 0.01);
    }

    #[test]
    fn one_percent_low_tracks_worst_frames() {
        // 99 fast frames (10ms) and 1 slow frame (100ms): 1% low = the slow one.
        let mut samples = vec![10.0_f32; 99];
        samples.push(100.0);
        let r = summarize("x", 0.016, 0, &samples, &[], &[], &[], &[]);
        assert!(r.avg_fps > r.one_percent_low_fps);
        assert!((r.one_percent_low_fps - 10.0).abs() < 0.5); // 1000/100 = 10 fps
        assert!((r.min_fps - 10.0).abs() < 0.5);
    }

    #[test]
    fn bound_timings_present_with_samples() {
        // 45ms GPU wait, 5ms CPU → gpu_bound_ratio = 45/50 = 0.9.
        let frame = vec![50.0_f32; 100];
        let gpu_wait = vec![45.0_f32; 100];
        let cpu = vec![5.0_f32; 100];
        let r = summarize("x", 0.016, 0, &frame, &gpu_wait, &cpu, &[], &[]);
        let b = r.bound.expect("bound timings should be present");
        assert_eq!(b.samples, 100);
        assert!((b.mean_gpu_wait_ms - 45.0).abs() < 0.01);
        assert!((b.mean_cpu_ms - 5.0).abs() < 0.01);
        assert!((b.gpu_bound_ratio - 0.9).abs() < 0.01);
    }

    #[test]
    fn record_frame_timing_only_while_running() {
        let raw = r#"{
            "start": {"x": 0.0, "y": 0.0, "z": 0.0},
            "segments": [{"keys": ["w"], "frames": 5}]
        }"#;
        let script: BenchmarkScript = serde_json::from_str(raw).unwrap();
        let mut bench = Benchmark {
            script,
            phase: Phase::Warmup { remaining: 3 },
            samples: Vec::new(),
            gpu_wait_samples: Vec::new(),
            cpu_samples: Vec::new(),
            sim_samples: Vec::new(),
            hud_samples: Vec::new(),
            world_overrides_taken: false,
            output_path: PathBuf::from("/dev/null"),
        };
        // Ignored during warmup.
        bench.record_frame_timing(45.0, 5.0, UpdateTimings::default(), 0.0);
        assert!(bench.gpu_wait_samples.is_empty());
        // Retained while running.
        bench.phase = Phase::Running {
            segment: 0,
            remaining: 5,
        };
        bench.record_frame_timing(45.0, 5.0, UpdateTimings::default(), 0.0);
        assert_eq!(bench.gpu_wait_samples.len(), 1);
        assert_eq!(bench.cpu_samples.len(), 1);
    }

    #[test]
    fn script_parses_with_defaults() {
        let raw = r#"{
            "start": {"x": 1.0, "y": 2.0, "z": 3.0},
            "segments": [{"keys": ["w"], "frames": 10}]
        }"#;
        let script: BenchmarkScript = serde_json::from_str(raw).unwrap();
        assert_eq!(script.name, "flythrough");
        assert!((script.fixed_dt - 1.0 / 60.0).abs() < 1e-6);
        assert_eq!(script.warmup_frames, 90);
        assert_eq!(script.segments.len(), 1);
        assert_eq!(script.segments[0].frames, 10);
    }
}
