# CLAUDE.md

## Project

Rust + wgpu 0.19 + winit 0.29 procedural terrain renderer. Flyable world with streaming chunks, camera movement, and day/night lighting.

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
```

### Visual feedback loop with `take_screenshot`

The debug API's `take_screenshot` command captures the current GPU frame to `captures/` (`latest.png` + timestamped history). Use this for a closed feedback loop: make a change, rebuild, take a screenshot, read `captures/latest.png` to verify the visual result, and iterate. The debug API is enabled by default on `127.0.0.1:7777`.

```bash
bun tools/debug-cli/cli.ts screenshot
# Then read captures/latest.png to see the result
```
