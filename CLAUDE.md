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

### Visual feedback loop with `take_screenshot`

The debug API's `take_screenshot` command captures the current GPU frame to `captures/` (`latest.png` + timestamped history). Use this for a closed feedback loop: make a change, rebuild, send a screenshot command via the debug API, read the resulting `captures/latest.png` to verify the visual result, and iterate. The debug API is enabled by default on `127.0.0.1:7777`.

```bash
curl -X POST http://127.0.0.1:7777/api/command \
  -H 'Content-Type: application/json' \
  -d '{"id":"ss-1","type":"take_screenshot"}'
```
