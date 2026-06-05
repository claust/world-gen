# Full Map Feature

**Status:** Implemented (v1, 2026-06-05)
**Shortcut:** `M` toggles a full-world map overlay (`M` or `Esc` closes it)

## As built

The v1 scope below shipped. Key files touched:

- [`src/app/mod.rs`](../src/app/mod.rs) — `map_open` + `world_map_tex` state on
  `AppState`; `toggle_map_overlay()`; the `Done` loading phase now *adopts*
  `loading_map_tex` into `world_map_tex` instead of dropping it; `render_map_ui()`
  draws the dimmed modal, upscaled biome map, and player arrow.
- [`src/app/event_loop.rs`](../src/app/event_loop.rs) — `M` intercept (native +
  wasm), camera frozen while the map is open, `Esc` closes the map first.
- [`src/debug_api/types.rs`](../src/debug_api/types.rs) +
  [`src/app/debug_commands.rs`](../src/app/debug_commands.rs) +
  [`tools/debug-cli/cli.ts`](../tools/debug-cli/cli.ts) — `press_key --key m` so the
  map can be driven from the debug CLI for the screenshot feedback loop.

**Verified** via the debug API: overlay opens/closes with `M` and `Esc`, stays
in-game on `Esc` (doesn't fall back to the menu), the red player arrow lands at the
correct wrapped world position, the corner minimap/HUD coexist, no log errors, all
54 tests pass.

**Observation:** at one pixel per 256 m chunk over the full 65 km world, the biome
map reads as fine speckle rather than smooth landmasses — the chunk *center* sample
crosses sea-level/biome thresholds frequently. This is faithful to the
loading-screen map (same data path), not a rendering bug, but it's the main reason
to consider the "heightmap relief shading" extension below for readability.

## Summary

Pressing **`M`** opens an ~80%-of-screen centered modal showing a top-down map of
the entire world. The world is finite and deterministic, so "the whole map" is a
bounded, fully-knowable set: a **256×256 grid of chunks** (256 m each → ~65.5 km ×
65.5 km, wrapping toroidally — see [`TORUS_WORLD.md`](../TORUS_WORLD.md)).

The map renders one pixel per chunk using the existing biome color function, with
the player's position + facing drawn on top and water/coastlines emphasized. The
whole world is revealed immediately (no fog of war in v1). Press `M` or `Esc` to
close.

## Decisions (v1 scope)

| Decision | Choice |
|---|---|
| Base visuals | **Per-chunk biome colors** (1 px/chunk, reuse loading-screen map) |
| Overlays | **Player position + facing**, **water & coastlines** |
| Interaction | **Static view** — open, look, close. No pan/zoom/teleport. |
| Fog of war | **Full reveal now.** Visited-only fog is a documented later extension. |

Explicitly **out of scope for v1** (candidate extensions below): heightmap relief
shading, house/settlement markers, chunk gridlines + coordinate readout, pan/zoom,
click-to-teleport, click-to-waypoint, fog of war.

## Why this is cheap: existing infrastructure

Most of the work is already done elsewhere in the codebase:

- **A full biome map is already computed during world load.** The "world being
  born" loading screen (`render_loading_ui` in [`src/app/mod.rs`](../src/app/mod.rs))
  paints a 256×256 biome-colored image into an egui texture as chunks generate.
  Per-chunk biome cells are stored in
  [`src/world_runtime/gen_progress.rs`](../src/world_runtime/gen_progress.rs)
  (`cells: Box<[AtomicU8]>`, row-major, with a `cell_color()` → RGBA mapping).
  **That image is essentially the map.**
- **Shared biome color function.** The corner minimap
  ([`src/renderer_wgpu/minimap_pass.rs`](../src/renderer_wgpu/minimap_pass.rs))
  and [`minimap_colors.rs`](../src/renderer_wgpu/minimap_colors.rs) already define
  `biome_color_rgba(height, moisture)`. Reuse it so the full map, the corner
  minimap, and the loading screen stay visually consistent.
- **UI is pure egui** (no web layer in-game). The 80% modal is the same pattern as
  the existing start menu ([`src/ui/start_menu.rs`](../src/ui/start_menu.rs)) and
  config panel ([`src/ui/config_panel.rs`](../src/ui/config_panel.rs)), rendered by
  [`src/renderer_wgpu/egui_pass.rs`](../src/renderer_wgpu/egui_pass.rs) on top of
  the 3D scene with `LoadOp::Load`.
- **`M` is currently unbound** — confirmed no existing keybinding.
- **Deterministic generation** means we *could* render unvisited chunks on demand,
  but for v1 we just reuse the already-built loading-screen map.

## World coordinate facts (for drawing the player marker)

- Chunk size: `CHUNK_SIZE_METERS = 256` ([`src/world_core/chunk.rs`](../src/world_core/chunk.rs))
- World size: `WORLD_SIZE_CHUNKS = 256` → 256×256 chunks, toroidal wrap
- World→chunk: `chunk = floor(world_xz / 256)` (see `world_to_chunk` in
  [`src/world_runtime/streaming.rs`](../src/world_runtime/streaming.rs))
- Sea level: `SEA_LEVEL = 40.0` — anything below is water
  ([`src/world_core/biome.rs`](../src/world_core/biome.rs))
- Biomes: Snow, Rock, Desert, Forest, Grassland (height + moisture thresholds)
- Player/camera world position is available from the camera controller; map north
  is +? (decide: convention is **+Z = south, north-up**, so map y grows downward
  with +Z — verify against camera yaw when wiring the facing arrow).

## Visual / UX spec

- **Modal:** centered egui window/panel sized to ~80% of the window, dimmed
  backdrop behind it (so the 3D scene shows through faintly), title like
  "World Map", and a close hint ("M / Esc to close").
- **Map image:** the 256×256 biome RGBA buffer, drawn with nearest-neighbor
  upscaling to fill the modal (blocky chunks are fine and readable). North-up,
  fixed origin — the whole world is shown at once, player is a marker on top rather
  than the map being recentered.
- **Water/coastlines:** ensure sub-sea-level chunks get a clear distinct water
  color via the shared color function; coastlines emerge naturally from the
  per-chunk coloring. (If contrast is weak at 1 px/chunk, consider a slightly
  bolder water tint just for the full map.)
- **Player marker:** a small arrow at the player's wrapped world position,
  rotated to camera yaw to show facing. Should stay legible on any biome color
  (e.g. white arrow with a dark outline).
- **Legend (nice-to-have, low cost):** a small biome color key in a corner of the
  modal.
- **While open:** the sim keeps running (day/night, live sim continue); camera
  movement input is frozen/swallowed while the modal is up (don't fly blind).
  `M` or `Esc` closes.

## Implementation sketch

Follow the F1 config-panel pattern closely.

1. **State:** add a `map_open: bool` (or a `Screen`/overlay flag) to `AppState`
   in [`src/app/mod.rs`](../src/app/mod.rs), plus a `toggle_map_overlay()` method.
   Guard it so it only opens during normal play (not on menu / loading / herbarium
   / editor) — mirror the existing `is_on_*` checks.
2. **Input:** in [`src/app/event_loop.rs`](../src/app/event_loop.rs), intercept
   `KeyCode::KeyM` on `ElementState::Pressed` before forwarding to the camera
   (same shape as the F1 special-case), call `toggle_map_overlay()`. When the map
   is open, route input to egui / swallow camera movement.
3. **Map texture:** reuse the loading-screen biome map. Retain the 256×256 RGBA
   buffer (and its egui texture handle) in `AppState` after loading completes
   instead of discarding it — it's only ~256 KB and terrain is static, so no
   regeneration is needed. (If retaining it is awkward, regenerate once on first
   `M` press by iterating canonical chunks through `biome_color_rgba`.)
4. **Render:** add a `render_map_ui()` egui pass (model it on `render_loading_ui`)
   that draws the dimmed backdrop, the upscaled map image, the player arrow
   (compute screen pos from player world pos → chunk → normalized → modal rect),
   and the close hint. Draw it after the 3D pass like other overlays.
5. **Player marker math:** `chunk_xy = world_xz / 256` (wrapped into `[0,256)`),
   `uv = chunk_xy / 256`, `screen = modal_rect.lerp(uv)`. Arrow rotation from
   camera yaw.

No new render pass or GPU work is strictly required — this is an egui overlay over
an existing texture.

## Testing / verification

- Manual: launch (`cargo run --release`), press `M`, confirm the map matches the
  loading-screen map and the corner minimap, player marker sits where expected,
  arrow points the right way as you turn, water reads clearly, `M`/`Esc` closes.
- Debug-CLI loop: `set_camera_position` to a few known spots, screenshot with the
  map open (`bun tools/debug-cli/cli.ts screenshot`), read `captures/latest.png`
  to verify marker placement. (Remember the ~2 s camera-settle gotcha after
  `set_camera_position`.)

## Later extensions (deliberately deferred)

- **Fog of war:** track which chunks have been loaded/seen (persistent visited
  set) and only reveal those; everything else drawn dark/unknown.
- **Heightmap relief shading** for a topographic look (sample finer height data,
  hillshade). Higher gen cost for unvisited areas.
- **Object markers:** houses/settlements (`HouseInstance` per chunk), points of
  interest. Only available for generated chunks unless generated on demand.
- **Grid + coordinates** readout for navigation/debug.
- **Interaction:** pan/zoom, click-to-set-waypoint (also shown on the corner
  minimap/HUD), click-to-teleport (decide whether teleport fits the intended
  play experience).
- **Recenter-on-player** mode that pans the toroidal world so the player is
  always centered.
