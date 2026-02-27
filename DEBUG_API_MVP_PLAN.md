# Debug API + Monitor MVP Plan

## Goal
Ship a minimum viable local debug interface for `world-gen` with:
- Local `HTTP + WebSocket` debug API in the game process
- One interactive command (`set_day_speed`)
- A separate Bun + React + Tailwind + shadcn monitor app

## Task Checklist

- [x] 1. Define MVP API contract and architecture boundaries  
  Done when: `v1` request/response/event schemas are written and frozen.

- [x] 2. Add a debug API feature flag in the game  
  Done when: debug API is disabled by default and can be enabled explicitly.

- [x] 3. Create `debug_api` transport adapter module in game  
  Done when: server lifecycle is isolated in infrastructure code.

- [x] 4. Add command bridge between server and game loop  
  Done when: commands are received over channels and applied on frame boundaries.

- [x] 5. Implement first runtime command: `set_day_speed`  
  Done when: value is validated, applied, and acked as success/error.

- [x] 6. Emit telemetry snapshots from game loop  
  Done when: frame/fps/hour/chunk/camera telemetry is published periodically.

- [x] 7. Implement minimal endpoints  
  Done when: `GET /health`, `GET /api/state`, `POST /api/command`, `GET /ws` work locally.

- [x] 8. Scaffold monitor app with Bun + React + Tailwind + shadcn  
  Done when: app runs via `bun dev` from `tools/debug-monitor`.

- [x] 9. Build minimal monitor UI  
  Done when: telemetry is visible and `set_day_speed` can be sent from UI.

- [x] 10. Add local dev workflow documentation  
  Done when: README has exact commands to run game + monitor together.

- [x] 11. Add basic safety and reliability checks  
  Done when: localhost-only binding, size limits, allowlist, and WS reconnect are in place.

- [x] 12. Run MVP acceptance pass  
  Done when: checklist passes and one verification screenshot is saved in `captures/`.

## Acceptance Criteria
- Game remains runnable without debug API enabled.
- Monitor can connect, show telemetry, and issue `set_day_speed`.
- Command acknowledgements include enough data to confirm when a change took effect.
- End-to-end workflow is documented and reproducible from a clean checkout.
