# World Gen

## Project Overview
Add your brief project purpose/vision here.

## What This Repository Contains
- A Rust + `wgpu` prototype for rendering a procedurally generated terrain world.
- A flyable MVP with streaming chunks, camera movement, and day/night lighting.
- A clean split between world core logic, runtime orchestration, and rendering adapters.

## Prerequisites
- macOS/Linux/Windows
- Rust toolchain (`rustc`, `cargo`)
- A GPU/driver setup that supports `wgpu`
- VS Code extensions (if using `F5`):
  - `rust-analyzer`
  - `CodeLLDB` (`vadimcn.vscode-lldb`)

## Project Setup
1. Clone the repository:
```bash
git clone https://github.com/claust/world-gen.git
cd world-gen
```
2. Install Rust (if needed):
```bash
curl https://sh.rustup.rs -sSf | sh -s -- -y
```
3. Load Cargo into your current shell:
```bash
. "$HOME/.cargo/env"
```
4. Enable repository hooks (checks Rust and debug-monitor TypeScript formatting + lint on staged files before commit):
```bash
git config core.hooksPath .githooks
```

## Run The App
1. Build and run in release mode:
```bash
cargo run --release
```
2. Optional: verify compile only:
```bash
cargo check
```

## Debugging
### Debug API + Monitor (MVP)
Run the game with the local debug API enabled:

```bash
cargo run --release -- --debug-api
```

Default bind is `127.0.0.1:7777`.

Optional overrides:

```bash
WORLD_GEN_DEBUG_API=true cargo run --release
cargo run --release -- --debug-api --debug-api-bind 127.0.0.1:9000
```

Run the monitor app (separate Bun + React + Tailwind + shadcn project):

```bash
cd tools/debug-monitor
bun install
bun dev
```

If debug API is on a non-default loopback port:

```bash
cd tools/debug-monitor
VITE_DEBUG_API_BASE=http://127.0.0.1:9000 bun dev
```

Available API routes:
- `GET /health`
- `GET /api/state`
- `POST /api/command`
- `GET /ws`

### Debug In VS Code (`F5`)
- Open this folder (`/Users/claus/Repos/world-gen`) as the workspace root.
- Select launch config `Debug world-gen`.
- Press `F5`.

## Screenshot Capture Workflow
Use this when you want to share the current rendered world state for review.

1. Capture one frame (put the `world-gen` window frontmost first):
```bash
./scripts/capture_world.sh
```

2. Capture multiple frames over time:
```bash
./scripts/capture_world_loop.sh 2 10
```
This captures every `2s` for `10` frames.

3. Output location:
- `captures/latest.png` (most recent)
- `captures/world-gen-YYYYMMDD-HHMMSS.png` (history)

## Controls
- `W/A/S/D`: move
- `Space`: move up
- `Left Shift`: move down
- `Left Ctrl`: speed boost
- Mouse: look around
- `Esc`: release mouse cursor
- `Left Click` (in window): capture mouse cursor again

## Current Status
- Architecture is now split into `world_core` (domain), `world_runtime` (use-cases/orchestration), and `renderer_wgpu` (render adapter).
- Streaming and world clock are active in the main app loop.
- Local debug API and monitor MVP are available for interactive telemetry, `set_day_speed`, and remote `W/A/S/D` movement commands.
