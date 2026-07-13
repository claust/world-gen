# CLAUDE.md

## Project

Rust + wgpu 27 + winit 0.29 procedural terrain renderer. Flyable world with streaming chunks, camera movement, and day/night lighting.

## Build & Run

```bash
cargo run --release          # run
cargo check                  # compile check only
```

Pre-commit hooks (enabled via `git config core.hooksPath .githooks`) run rustfmt and clippy on staged files.

## Architecture

Three-layer split:

- **`src/world_core/`** — Domain logic: chunk/terrain/biome generation, heightmaps, world time. Pure data, no rendering.
- **`src/world_runtime/`** — Orchestration: chunk streaming, runtime state management around the camera.
- **`src/renderer_wgpu/`** — GPU rendering adapter: all wgpu code lives here.

### Renderer internals (`renderer_wgpu/`)

- `GpuContext` — wraps wgpu device/queue/surface/config/size
- `Material` — bind group layout with group 0 (per-frame: view_proj, camera, time) and group 1 (per-material: lighting)
- `TerrainPass` — compute-generated terrain mesh, 129×129 grid per chunk, 256m chunks, shared index buffer
- `InstancedPass` — instanced rendering of prototype meshes (box, octahedron, house) for trees/houses
- `WorldRenderer` — orchestrates passes, manages chunk GPU state

### Debug API (`src/debug_api/`)

HTTP + WebSocket server (axum) exposing telemetry and commands. Companion monitor app lives in `tools/debug-monitor/` (Bun + React).

### Debug CLI (`tools/debug-cli/cli.ts`)

Bun+TypeScript CLI for sending debug API commands and receiving results as JSON. Preferred over raw curl — it handles the HTTP POST + WebSocket response flow in one call.

```bash
bun tools/debug-cli/cli.ts state                              # get telemetry
bun tools/debug-cli/cli.ts screenshot                          # capture frame
bun tools/debug-cli/cli.ts find_nearest --kind house           # find nearest object
bun tools/debug-cli/cli.ts look_at --id house-0_0-3 --distance 20  # inspect object
bun tools/debug-cli/cli.ts set_camera_position --x 100 --y 150 --z 100
bun tools/debug-cli/cli.ts set_camera_look --yaw 1.5 --pitch -0.3
bun tools/debug-cli/cli.ts set_day_speed --value 0.1
bun tools/debug-cli/cli.ts move --key w --duration 500
bun tools/debug-cli/cli.ts save                                # save camera + plant state
```

### Multiple instances (`tools/launch.ts`)

By default the game can't run twice on one machine: the debug API port (7777)
and the on-disk state (`save.json`, `config.json`, `plants.bin`, `captures/`)
collide. The launcher fixes both.

```bash
bun tools/launch.ts                 # build + launch a uniquely-named instance
bun tools/launch.ts --name alpha    # launch the named instance "alpha"
bun tools/launch.ts --no-build      # reuse the existing release binary
bun tools/launch.ts -- --benchmark benchmarks/smoke.json  # pass args to the game
bun tools/launch.ts list            # list live instances (prunes dead records)
```

- **Port:** the game is started with `--debug-api-bind 127.0.0.1:0`, so the OS
  assigns a free port (no scan, no race). The launcher reads the real port back
  from the `debug api listening on …` startup log and prints a handshake line:
  `[launch] ready {"name":…,"port":…,"api":…}`.
- **State:** `--instance-name <name>` roots all on-disk state under
  `instances/<name>/` (gitignored). `herbarium.json`/`config.json` are seeded
  from the repo root on first launch so a new instance isn't a blank slate.
- **Discovery:** each running instance is recorded at
  `instances/<name>/instance.json` (name, pid, port, dirs). The record is pruned
  when the instance exits or is found dead; the state dir is kept.

The game also takes `--instance-name <name>` directly (or the `WORLD_GEN_INSTANCE`
env var) without the launcher, if you want to assign the port yourself.

`tools/debug-cli/cli.ts` accepts `--name <name>` to resolve a running instance's
port from its registry file, so you don't have to track ports by hand:

```bash
bun tools/debug-cli/cli.ts state --name alpha       # target instance "alpha"
bun tools/debug-cli/cli.ts screenshot --name beta   # → instances/beta/captures/
```

### FPS benchmark (`src/app/benchmark.rs`, `tools/bench.ts`)

Deterministic FPS benchmark mode. The camera replays a scripted flythrough on a
fixed timestep so the rendered workload is identical across runs; the surface is
switched to a non-vsync present mode to measure true throughput. Results are
written to `benchmarks/latest.json` (avg/min/max/1%-low FPS, mean/p50/p95/p99
frame times) and diffed against `benchmarks/baseline.json`. The `bun tools/bench.ts`
wrapper exits non-zero on a >5% FPS drop, so it can gate CI — but the current CI
workflow (`.github/workflows/ci.yml`) runs only fmt/clippy/build and does not yet
invoke the benchmark; run it locally.

```bash
bun tools/bench.ts                       # build + run default flythrough
bun tools/bench.ts benchmarks/smoke.json # shorter (~10s) smoke run
bun tools/bench.ts --no-build            # reuse existing binary
bun tools/bench.ts --baseline            # save this run as the baseline
cargo run --release -- --benchmark benchmarks/flythrough.json  # without the wrapper
```

The report also includes a `bound` block (`gpu_bound_ratio`, `mean_gpu_wait_ms`,
`mean_cpu_ms`) derived from CPU-side timers: `gpu_wait_ms` is the swapchain-acquire
stall (≈ GPU frame time when GPU-bound) and `cpu_ms` is CPU-active work. This works
on every backend, unlike GPU timestamp queries, which do not bracket fragment work
on Apple Silicon (tile-based deferred GPUs) — pass-boundary timestamps measure only
the tiling phase and encoder-level `write_timestamp` writes zeros on Metal.

**Hitch attribution.** Every recorded frame also carries the `WorldRuntime::update`
phase breakdown (growth / spread / streaming / refresh / census) plus the HUD stats
scan. Frames over 50 ms CPU are listed in the report's `worst_hitches` with those
per-phase costs, and `phases` sums each phase over the run — so a periodic stall
can be pinned to the sim pass that causes it.

**Reproducing plant-sim stalls.** Benchmark mode normally starts a fresh world,
which never shows them (a young, small population has no lifecycle events due).
Script fields to control the sim state:

- `"day_speed": 0.6` — override the clock speed (deterministic under the fixed
  timestep); one growth tick per sim-hour ⇒ every `1/(fixed_dt·day_speed)` frames.
- `"start_total_hours": 20004.0` — jump the clock after load; base plants are born
  at hour 0, so this ages the world (still far smaller than a long-played save).
- `"resume": true` — resume the real save (e.g. `benchmarks/hud_stall.json`, which
  circles in place on the resumed world). A long-played save carries tens of
  millions of plants — the state where the hourly growth pass and census stall the
  frame. Machine-specific, so for diagnosis, not CI baselines. Saving is disabled
  throughout benchmark mode; the resumed state is never written back.

`sim_bench` (headless, no GPU) mirrors this for fast iteration:
`cargo run --release --bin sim_bench -- --state-dir . --warmup-steps 500 --steps 2000`
replays the real state dir read-only and prints per-step spikes with the same phase
split. Note that on an old save the population drops sharply on the first growth
tick — that's by design, not a restore bug: the app and sim_bench both restore
the full save (base + spread), then the first tick reaps every plant whose
analytic despawn already passed (on the 206k-hour save, ~11.5M of the 23.9M
loaded plants — most of the hour-0 base cohort is long dead). Since the
build-time prewarm pass, that reap happens during load, so reported populations
are post-reap everywhere.

### Visual feedback loop with `take_screenshot`

The debug API's `take_screenshot` command captures the current GPU frame to `captures/` (`latest.png` + timestamped history). Use this for a closed feedback loop: make a change, rebuild, take a screenshot, read `captures/latest.png` to verify the visual result, and iterate. The debug API is enabled by default on `127.0.0.1:7777`.

```bash
bun tools/debug-cli/cli.ts screenshot
# Then read captures/latest.png to see the result
```
