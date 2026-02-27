# Debug API v1 Contract

## Scope
This contract defines the minimum viable local debug interface for `world-gen`.

- Transport: HTTP + WebSocket
- Binding: `127.0.0.1:7777` by default
- API version: `v1`
- Command allowlist: `set_day_speed`, `set_move_key`

## Architecture Boundaries

- `world_core`: pure domain logic; no transport or serialization concerns.
- `world_runtime`: simulation/use-cases; handles command intent (e.g. set day speed).
- `renderer_wgpu`: rendering adapter; no HTTP/WS code.
- `debug_api`: infrastructure adapter for HTTP/WS transport and command/event bridging.
- `main`: composition root; owns frame loop and applies inbound commands at frame boundaries.

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
Enqueues one command to be applied by the game loop on a frame boundary.

Request:

```json
{
  "id": "cmd-1",
  "type": "set_day_speed",
  "value": 0.12
}
```

```json
{
  "id": "cmd-2",
  "type": "set_move_key",
  "key": "w",
  "pressed": true
}
```

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

## WebSocket Endpoint

### `GET /ws`
Streams events as JSON text frames.

On connect, server sends latest telemetry (if available), then pushes updates.

Event envelope uses tagged payloads:

```json
{ "type": "telemetry", "payload": { ... } }
```

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

## Command Handling Semantics

- HTTP `POST /api/command` only enqueues commands.
- Commands are applied in the main frame loop (deterministic boundary).
- Each applied command emits one `command_applied` event with frame index.
- The telemetry stream reflects the updated value after application.
- `set_move_key` is a press/release command for `w|a|s|d`; movement continues while key is pressed.

## Safety/Operational Constraints

- Loopback-only binding (`127.0.0.1`); non-loopback rejected.
- Max command payload size: 8 KiB.
- Command type allowlist enforced by request enum.
- UI should reconnect WS automatically with retry backoff.
