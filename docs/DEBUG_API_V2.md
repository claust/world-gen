# Debug API v2 Contract

## Scope

Local debug interface for `world-gen`. Provides telemetry, camera control, world inspection, and screenshot capture.

- Transport: HTTP + WebSocket
- Binding: `127.0.0.1:7777` by default
- API version: `v1` (wire format unchanged)

## Configuration

The debug API can be enabled via:

- CLI flag: `--debug-api`
- CLI bind override: `--debug-api-bind <addr:port>`
- Env var: `WORLD_GEN_DEBUG_API=1` (truthy values: `1`, `true`, `yes`, `on`)

Enabled by default in debug builds.

## Architecture Boundaries

- `world_core`: pure domain logic; no transport or serialization concerns.
- `world_runtime`: simulation/use-cases; handles command intent (e.g. set day speed).
- `renderer_wgpu`: rendering adapter; no HTTP/WS code.
- `debug_api`: infrastructure adapter for HTTP/WS transport and command/event bridging.
- `main`/`app`: composition root; owns frame loop and applies inbound commands at frame boundaries.

Dependency direction:

- `main` -> `world_runtime`, `renderer_wgpu`, `debug_api`
- `world_runtime` -> `world_core`
- `renderer_wgpu` -> `world_core`
- `debug_api` -> shared DTOs/bridge only

## HTTP Endpoints

### `GET /health`

Returns server health and runtime mode.

Response `200`:

```json
{
  "status": "ok",
  "api_version": "v1",
  "debug_api_enabled": true
}
```

### `GET /api/state`

Returns latest telemetry snapshot.

Response `200`:

```json
{
  "api_version": "v1",
  "telemetry": {
    "frame": 120,
    "frame_time_ms": 9.9,
    "fps": 101.0,
    "hour": 9.8,
    "day_speed": 0.04,
    "camera": { "x": 96.0, "y": 150.0, "z": 16.0, "yaw": 1.02, "pitch": -0.38 },
    "chunks": { "loaded": 9, "pending": 0, "center": [0, 0] },
    "timestamp_ms": 1730000000000
  }
}
```

### `POST /api/command`

Enqueues one command to be applied by the game loop on the next frame boundary.

Immediate response `202`:

```json
{
  "api_version": "v1",
  "id": "cmd-1",
  "status": "accepted"
}
```

Validation error response `400`:

```json
{
  "api_version": "v1",
  "error": "invalid_command",
  "message": "day speed must be between 0.0 and 2000.0"
}
```

## Commands

All commands use `POST /api/command` with a JSON body containing `id` (string) and `type` (command name). Fields are snake_case.

### `set_day_speed`

Set the day/night cycle speed.

```json
{ "id": "cmd-1", "type": "set_day_speed", "value": 0.12 }
```

| Field   | Type  | Required | Description                        |
|---------|-------|----------|------------------------------------|
| `value` | f32   | yes      | Speed multiplier (0.0 to 2000.0)   |

Response payload includes `day_speed` confirming the new value.

### `set_move_key`

Simulate a movement key press or release. Movement continues while the key is held.

```json
{ "id": "cmd-2", "type": "set_move_key", "key": "w", "pressed": true }
```

| Field     | Type   | Required | Description                            |
|-----------|--------|----------|----------------------------------------|
| `key`     | string | yes      | One of: `w`, `a`, `s`, `d`, `up`, `down` |
| `pressed` | bool   | yes      | `true` to press, `false` to release    |

### `set_camera_position`

Teleport the camera to an absolute world position.

```json
{ "id": "cmd-3", "type": "set_camera_position", "x": 100.0, "y": 200.0, "z": 50.0 }
```

| Field | Type | Required | Description       |
|-------|------|----------|-------------------|
| `x`   | f32  | yes      | World X position  |
| `y`   | f32  | yes      | World Y (height)  |
| `z`   | f32  | yes      | World Z position  |

### `set_camera_look`

Set the camera orientation.

```json
{ "id": "cmd-4", "type": "set_camera_look", "yaw": 1.5, "pitch": -0.3 }
```

| Field   | Type | Required | Description                              |
|---------|------|----------|------------------------------------------|
| `yaw`   | f32  | yes      | Horizontal angle (radians)               |
| `pitch` | f32  | yes      | Vertical angle (radians, clamped to safe range) |

### `find_nearest`

Find the nearest world object of a given kind relative to the current camera position.

```json
{ "id": "cmd-5", "type": "find_nearest", "kind": "house" }
```

| Field  | Type   | Required | Description                        |
|--------|--------|----------|------------------------------------|
| `kind` | string | yes      | One of: `house`, `tree`, `fern`    |

Response payload includes `object_id` and `object_position` (`[x, y, z]`) of the nearest match.

### `look_at_object`

Position and orient the camera to look at a specific world object by ID.

```json
{ "id": "cmd-6", "type": "look_at_object", "object_id": "house-0_0-3", "distance": 15.0 }
```

| Field       | Type   | Required | Description                              |
|-------------|--------|----------|------------------------------------------|
| `object_id` | string | yes      | Object ID (from `find_nearest` results)  |
| `distance`  | f32    | no       | Camera distance from object (default: auto) |

Response payload includes `object_id` and `object_position`.

### `take_screenshot`

Capture the current GPU frame. Saves to `captures/latest.png` (plus a timestamped copy).

```json
{ "id": "cmd-7", "type": "take_screenshot" }
```

No additional fields.

### `press_key`

Simulate a key press (toggle actions).

```json
{ "id": "cmd-8", "type": "press_key", "key": "f1" }
```

| Field | Type   | Required | Description                      |
|-------|--------|----------|----------------------------------|
| `key` | string | yes      | One of: `f1`, `escape`           |

Key effects:
- `f1`: Toggle the egui config panel overlay
- `escape`: Toggle cursor grab/release

## WebSocket Endpoint

### `GET /ws`

Streams events as JSON text frames.

On connect, the server sends the latest telemetry snapshot (if available), then pushes updates continuously.

Event envelope uses tagged payloads:

#### `telemetry`

```json
{ "type": "telemetry", "payload": { ... } }
```

Payload matches the `telemetry` object from `GET /api/state`.

#### `command_applied`

Emitted once per command after it is applied on a frame boundary.

```json
{
  "type": "command_applied",
  "payload": {
    "id": "cmd-1",
    "frame": 121,
    "ok": true,
    "message": "day speed set",
    "day_speed": 0.12
  }
}
```

| Field             | Type     | Always | Description                              |
|-------------------|----------|--------|------------------------------------------|
| `id`              | string   | yes    | Command ID echoed back                   |
| `frame`           | u64      | yes    | Frame index when command was applied     |
| `ok`              | bool     | yes    | Whether the command succeeded            |
| `message`         | string   | yes    | Human-readable result message            |
| `day_speed`       | f32      | no     | Present on `set_day_speed` success       |
| `object_id`       | string   | no     | Present on `find_nearest`/`look_at_object` |
| `object_position` | [f32; 3] | no     | Present on `find_nearest`/`look_at_object` |

## Command Handling Semantics

- `POST /api/command` only enqueues; it does not execute immediately.
- Commands are applied in the main frame loop at deterministic frame boundaries.
- Each applied command emits exactly one `command_applied` event with the frame index.
- The telemetry stream reflects updated values after application.

## CLI Usage

The debug CLI (`tools/debug-cli/cli.ts`) wraps the HTTP+WS flow into single commands:

```bash
bun tools/debug-cli/cli.ts <command> [options]
```

| Command                | Flags                                          | Description                     |
|------------------------|------------------------------------------------|---------------------------------|
| `state`               | —                                              | Get current telemetry           |
| `screenshot`           | —                                              | Capture frame to `captures/`    |
| `set_day_speed`        | `--value <n>`                                  | Set day/night speed             |
| `set_camera_position`  | `--x <n> --y <n> --z <n>`                     | Teleport camera                 |
| `set_camera_look`      | `--yaw <n> --pitch <n>`                        | Set camera orientation          |
| `find_nearest`         | `--kind <house\|tree\|fern>`                   | Find nearest object             |
| `look_at`              | `--id <object_id> [--distance <n>]`            | Look at object                  |
| `move`                 | `--key <w\|a\|s\|d\|up\|down> [--duration <ms>]` | Move camera (press+hold+release) |
| `press_key`            | `--key <f1\|escape>`                           | Press a key                     |

All commands accept `--api <url>` to override the default base URL (`http://127.0.0.1:7777`).

The `move` command is a convenience wrapper — it sends `set_move_key` with `pressed: true`, waits for `--duration` ms (default: 200), then sends `pressed: false`.

## Safety/Operational Constraints

- Loopback-only binding (`127.0.0.1`); non-loopback addresses are rejected.
- Max command payload size: 8 KiB.
- Command type allowlist enforced by the request enum (unknown types are rejected).
- WebSocket clients should reconnect automatically with retry backoff.
