# World Gen

## Project Overview
A procedurally generated 3D world with mountains, forests, rivers, and other natural landscapes — built entirely from scratch in Rust using `wgpu`. The vision is to create a beautiful, explorable open world with rich biomes and terrain variety, all generated on the fly.

This is an educational project focused on learning Rust and building a homemade 3D engine from the ground up — no Unity, no Unreal, no off-the-shelf game engine. Everything from the rendering pipeline to the terrain generation is written using LLM's.

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
### Debug API + CLI
Run the game with the debug API enabled:

```bash
cargo run --release -- --debug-api
```

Default bind is `127.0.0.1:7777`. Optional override:

```bash
cargo run --release -- --debug-api --debug-api-bind 127.0.0.1:9000
```

Use the debug CLI (requires [Bun](https://bun.sh)) to interact with the running game:

```bash
bun tools/debug-cli/cli.ts state                                    # get telemetry
bun tools/debug-cli/cli.ts screenshot                                # capture frame to captures/
bun tools/debug-cli/cli.ts set_day_speed --value 0.1                 # set day/night cycle speed
bun tools/debug-cli/cli.ts set_camera_position --x 100 --y 150 --z 100  # teleport camera
bun tools/debug-cli/cli.ts set_camera_look --yaw 1.5 --pitch -0.3   # set camera orientation
bun tools/debug-cli/cli.ts find_nearest --kind house                 # find nearest object
bun tools/debug-cli/cli.ts look_at --id house-0_0-3 --distance 20   # look at a specific object
bun tools/debug-cli/cli.ts move --key w --duration 500               # move camera
```

If the debug API is on a non-default port, pass `--api http://127.0.0.1:9000`.

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
