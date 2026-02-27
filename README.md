# World Gen

## Project Overview
Add your brief project purpose/vision here.

## What This Repository Contains
- A Rust + `wgpu` prototype for rendering a procedurally generated terrain world.
- A minimal flyable MVP (single generated chunk, camera movement, simple lighting/sky).
- A modular code layout for future phases (biomes, vegetation, streaming, time-of-day, debug UI).

## Prerequisites
- macOS/Linux/Windows
- Rust toolchain (`rustc`, `cargo`)
- A GPU/driver setup that supports `wgpu`
- VS Code extensions (if using `F5`):
  - `rust-analyzer`
  - `CodeLLDB` (`vadimcn.vscode-lldb`)

## Setup
1. Clone the repository:
```bash
git clone <your-repo-url>
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

## Run
1. Build and run in release mode:
```bash
cargo run --release
```
2. Optional: verify compile only:
```bash
cargo check
```

## Debug In VS Code (`F5`)
- Open this folder (`/Users/claus/Repos/world-gen`) as the workspace root.
- Select launch config `Debug world-gen`.
- Press `F5`.

## Screenshot Workflow For Fast Feedback
Use this when you want to share the current rendered world state for review.

1. Grant macOS permissions for capture tools (needed by Peekaboo):
```bash
peekaboo list permissions
```
If missing, enable:
- `System Settings > Privacy & Security > Screen Recording`
- `System Settings > Privacy & Security > Accessibility`

2. Capture one frame (put the world-gen window frontmost first):
```bash
./scripts/capture_world.sh
```

3. Capture multiple frames over time:
```bash
./scripts/capture_world_loop.sh 2 10
```
This captures every `2s` for `10` frames.

4. Output location:
- `captures/latest.png` (most recent)
- `captures/world-gen-YYYYMMDD-HHMMSS.png` (history)

## Controls
- `W/A/S/D`: move
- `Space`: move up
- `Left Shift`: move down
- `Left Ctrl`: speed boost
- Mouse: look around
- `Esc`: quit

## Project Structure
```text
src/
  main.rs
  world_gen/
    heightmap.rs
    biome.rs
    vegetation.rs
    chunk.rs
  renderer/
    terrain.rs
    instanced.rs
    sky.rs
    camera.rs
    pipeline.rs
    shaders/
      terrain.wgsl
  world/
    streaming.rs
    time.rs
```

## Current Status
- MVP is focused on visualizing terrain quickly with minimal complexity.
- Streaming, full biome material blending, vegetation instancing, and richer day/night are scaffolded for upcoming phases.
