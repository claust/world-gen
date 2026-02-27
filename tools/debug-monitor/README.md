# World Gen Debug Monitor

A local web UI for observing and controlling a running `world-gen` instance through its debug API.

## Overview
The debug monitor is a Bun + React + Vite app used during development to:
- show live telemetry from the game over WebSocket,
- display key runtime state (frame, FPS, clock, camera, chunk streaming),
- send debug commands (`set_day_speed`, `set_move_key`) and show command acknowledgements.

The monitor is intended for local development only and assumes a loopback debug API.

## What It Connects To
The monitor talks to the game debug API endpoints:
- `GET /api/state` for initial state snapshot,
- `POST /api/command` to send commands,
- `GET /ws` for live telemetry and command acks,
- `GET /health` for service checks (useful for manual verification).

Default API base URL is:
- `http://127.0.0.1:7777`

Override with:
- `VITE_DEBUG_API_BASE=http://127.0.0.1:<port>`

## Run Locally
From this directory (`tools/debug-monitor`):

```bash
bun install
bun dev
```

Then open:
- `http://127.0.0.1:4173`

If your game debug API uses a different port:

```bash
VITE_DEBUG_API_BASE=http://127.0.0.1:9000 bun dev
```

## Typical Dev Workflow
1. Start `world-gen` with debug API enabled (from repo root):
```bash
cargo run --release -- --debug-api
```
2. Start this monitor app.
3. Confirm `WS connected` appears in the UI.
4. Use the day speed control to send `set_day_speed`.
5. Use the WASD controls (or keyboard) to send `set_move_key` press/release commands.

## Scripts
- `bun dev`: run Vite dev server
- `bun build`: type-check and build production assets
- `bun preview`: preview production build
- `bun lint`: run oxlint with type-aware checks
- `bun format` / `bun format:check`: format/check formatting
- `bun test:e2e`: run Playwright end-to-end test

## E2E Test Notes
The Playwright test expects:
- monitor UI reachable at `MONITOR_BASE_URL` (default `http://127.0.0.1:4173`),
- game debug API reachable at `DEBUG_API_BASE` (default `http://127.0.0.1:7777`).

Start both apps before running:

```bash
bun test:e2e
```
