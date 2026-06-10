# Naturalistic Grass — Implementation Plan

Layered hybrid grass rendering: instanced, wind-swaying procedural grass tufts
near the camera, thinning smoothly with distance, blending into an improved
far-field terrain shader. Everything is procedural (no external assets) and
deterministic from the world seed, consistent with how trees, shrubs, and the
terrain atlas are generated.

## Goals

1. **Near field (0–~35 m):** full-density instanced 3D grass tufts with
   per-blade wind sway, varied heights, rooted in the terrain and tinted to
   match the ground color so they read as part of the terrain.
2. **Mid field (~35–90 m):** the same tufts, progressively thinned and
   height-faded so there is no visible pop or hard line.
3. **Far field (beyond grass range):** improved terrain ground shading —
   large-scale color variation that breaks the uniform green carpet and ties
   the whole grass plain together. The blade tint and terrain albedo share the
   same variation so the near/far transition is invisible.

## Non-goals (this pass)

- No player/wind interaction with grass (no displacement around the camera).
- No grass in the shadow map (grass *receives* shadows but does not cast).
- No herbarium/species integration — grass is a render-layer effect, not a
  plant in the ecology sim. It is never persisted and never affects `gen_key`.
- No config/UI surface; tunables are constants in one place.
- Tree sway is out of scope (separate follow-up if wanted).

## Architecture

Grass is a **camera-window view layer**, not chunk content. A 256 m chunk at
~0.5 m tuft spacing would be ~260k instances — storing that for every loaded
chunk is wasteful when grass is only visible within ~90 m. Instead, the world
is divided into **32 m grass tiles** (8×8 per chunk, chunk-aligned so a tile
never straddles chunks). A tile cache around the camera generates instance
buffers lazily and drops them with hysteresis as the camera moves.

Placement is **pure domain logic in `world_core`** (deterministic from seed +
tile coordinate + terrain fields, same hashing style as `FloraLayer`); GPU
buffers, the tile cache, and rendering live in a new **`GrassPass`** in
`renderer_wgpu`, following the existing pass pattern.

```
world_core/content/grass.rs      generate_grass_tile(...) -> Vec<GrassTuft>   (pure, tested)
renderer_wgpu/grass_pass.rs      GrassPass: tile cache, tuft mesh, pipeline, render
renderer_wgpu/shaders/grass.wgsl wind sway vertex shader + lit fragment shader
renderer_wgpu/shaders/lighting.wgsl   + shared ground_macro_variation() helper
renderer_wgpu/shaders/terrain.wgsl    + apply macro variation to ground albedo
```

### Layer 1 — placement (`world_core/content/grass.rs`)

`generate_grass_tile(seed, biome_config, sea_level, chunk_coord, tile_index,
terrain: &ChunkTerrain) -> Vec<GrassTuft>`

- Jittered grid inside the 32 m tile, **0.5 m spacing** (4 096 cells/tile),
  hashed with `hash4` on the **canonical chunk** + cell id (seed offset 9000,
  distinct from tree/shrub/aquatic grids), exactly like `FloraLayer`.
- Per-cell guards, mirroring flora: skip `height < sea_level + 0.15`, skip
  river cells (`wetness > MAX_PLANTABLE_WETNESS`), skip below the river water
  sheet (`height < river_surface`), skip steep slopes (`slope > 0.7`).
- Biome density via `biome::classify(height, moisture, biome_config)`:
  - Grassland: `0.55 + moisture * 0.40` (dense lawn)
  - Forest: `0.30` (sparser under-story)
  - Desert: `0.04` (occasional dry tufts; they tint sandy automatically)
  - Rock/Snow: `0.0`
- Each surviving cell yields a `GrassTuft { position (root, sunk 4 cm into the
  ground), yaw, scale (height: 0.75 + moisture·0.5 ± hash jitter; width
  follows), fade_key (per-tuft hash in 0..1), biome }`.
- Output is **sorted by `fade_key` descending** so any buffer prefix is a
  spatially uniform random subset — distance thinning is then just drawing a
  prefix (`instance_count` scaling), no re-bucketing.
- Unit tests: determinism, sea-level/river guards, sort order, density by biome.

### Layer 2 — GPU pass (`renderer_wgpu/grass_pass.rs`)

**Tuft prototype mesh** (one shared mesh, built once in Rust):
- 5 blades per tuft; each blade is a tapered two-segment strip: 5 vertices
  (2+2+1), 3 triangles → 25 verts / 15 tris per tuft.
- Reuses `geometry::Vertex` (position, normal, color) so `upload_prototype`
  works unchanged. The `color` channel is repurposed as packed blade data:
  `r = bend weight` (0 root → 1 tip, quadratic-ish along the blade),
  `g = per-blade phase` (0..1), `b = ambient-occlusion factor` (darker at the
  tuft base). Normals lean out from the tuft axis, mixed toward +Y in the
  shader for soft terrain-like lighting.
- Blades fan out from the tuft center with varied lean/height so a single
  prototype doesn't read as a repeated stamp once instance yaw/scale vary.

**Instances** reuse `InstanceData` (52 B) and `upload_instances`:
- `position` = root, `rotation_y` = yaw, `scale` = (w, h, w),
- `color.rgb` = **root tint**: the same procedural atlas color the terrain
  fragment shader samples at that world position (`terrain_texture` exposes
  the per-tile color functions; tint = srgb-decode of the atlas texel so it
  matches what the GPU samples from the `Rgba8UnormSrgb` atlas), darkened
  ~10 % so blade roots sit *into* the ground rather than on it,
- `color.a` = fade key (drives pop-free shader-side thinning), `tilt` unused.

**Tile cache** (`HashMap<IVec2 /*world tile coord*/, GrassTile>`):
- Wanted set: tiles whose center is within `GRASS_FAR + 16 m` of the camera
  and whose owning chunk is loaded; tiles dropped beyond `GRASS_FAR + 48 m`
  (hysteresis) or when their chunk unloads.
- Generation budget: ≤ 6 tiles per frame (closest first) so initial load and
  fast flight never hitch.
- `sync(device, chunks, camera_pos)` called from `WorldRenderer::sync_chunks`
  (camera position from the previous `update_frame` is fine — tiles have tens
  of meters of margin).

**Render** (in `render_scene`, after `instanced`, before river/water — opaque,
depth-tested, so most of its fill is depth-rejected by terrain/trees already
drawn):
- Pipeline: `create_billboard_pipeline` (cull off — blades are two-sided,
  opaque, depth-writing; no alpha test needed). Bind groups identical to
  `InstancedPass` (0 = frame, 1 = material, 2 = shadow map), so fog, hemisphere
  ambient, and shadow receive all come from `lighting.wgsl` for free.
- Per tile: frustum-cull via tile AABB (terrain min/max height), compute
  `keep = keep_fraction(distance)` (1.0 inside 35 m, cubic falloff to 0 at
  90 m), draw prefix `instance_count = ceil(total · min(keep · 1.15, 1))`
  — the 15 % margin lets the shader own the actual fade so changing counts
  never pops.
- Not rendered into the shadow map; skipped entirely in `render_depth`.
- Stats: tile count + drawn instances exposed through `RendererStats`.

### Layer 3 — shaders

**`grass.wgsl`** (concatenated after `lighting.wgsl` like every lit pass):
- Vertex: instance transform (yaw + scale + translate) → **wind sway**:
  - primary traveling wave: `sin(dot(world_xz, WIND_DIR) · 0.35 − time · 1.8)`,
  - gust modulation: slow value-noise sampled at `world_xz · 0.02 − time · 0.35`
    scaling amplitude 0.4–1.6 so the field rolls in irregular waves,
  - per-blade phase (`color.g`) + per-tuft hash de-synchronize neighbors,
  - displacement along wind dir (plus slight perpendicular wobble), weighted by
    `bend_weight²` so roots stay planted; tip Y dips by ~bend²·0.3·height to
    approximate constant blade length.
  - **distance fade**: per-instance keep test against `keep_fraction(dist)`
    using `color.a` — instances past the threshold smoothly scale height to 0
    (and a global height taper toward `GRASS_FAR` softens the horizon of the
    grass field).
- Fragment: albedo = root tint → tip: lightened (+25 %) and warmed slightly
  (`mix` by bend weight), multiplied by the shared `ground_macro_variation()`;
  base darkened by the AO factor (`color.b`). Lighting = hemisphere ambient +
  N·L sun with the blade normal mixed 60 % toward +Y (soft, terrain-like — no
  harsh per-blade speculars), shadow-mapped like terrain, `scene_fog` at the end.

**`lighting.wgsl`**: add a small hash/value-noise pair and
`ground_macro_variation(world_xz: vec2<f32>) -> vec3<f32>` — two octaves of
low-frequency noise (~0.013 and ~0.045 cycles/m) producing patchy ±10 %
brightness with a subtle dry-yellow hue shift in the low patches. Pure
function, no new bindings.

**`terrain.wgsl`**: multiply the blended biome albedo by
`ground_macro_variation(world_pos.xz)` scaled by `(1 − wetness)` (riverbeds
stay water-tinted). Because the *same* multiplier hits grass blades, near
blades and far ground shift hue together.

## Wiring

- `WorldRenderer::new` gains `seed: u32` + `biome_config: BiomeConfig`
  (forwarded from app config at construction; both call sites updated).
- `WorldRenderer::sync_chunks` → `self.grass.sync(...)` (uses
  `self.camera_position`); `clear_chunks` empties the tile cache via the
  normal retain path; `render_scene` draws the pass.
- New modules registered in `world_core/content/mod.rs` and
  `renderer_wgpu/mod.rs`.
- Works on wasm unchanged (no native-only APIs; pure CPU gen + standard wgpu).

## Tunables (constants in `grass_pass.rs` / `grass.rs`)

| Constant | Value | Meaning |
|---|---|---|
| `TILE_SIZE` | 32 m | grass tile side (8×8 per chunk) |
| `SPACING` | 0.5 m | tuft grid spacing |
| `GRASS_FULL` | 35 m | full density radius |
| `GRASS_FAR` | 90 m | grass horizon (keep → 0, cubic falloff) |
| `GRASS_LOD_DISTANCE` | 40 m | tiles beyond this draw the 2-blade LOD mesh |
| `TILES_PER_FRAME` | 6 | tile generation budget |
| `BLADES_PER_TUFT` / `_LOD` | 5 / 2 | prototype blades (LOD blades 1.6× wider) |
| `WIND_DIR/SPEED/STRENGTH` | (0.78, 0.62) / 1.8 / ~0.12 m | sway model |

The first tuning round taught two lessons, baked in above: distant blades are
subpixel, so what costs on a tile-based GPU is raw micro-triangle count — hence
the cubic (not quadratic) thinning and the wide 2-blade LOD mesh past 40 m.

## Verification results (2026-06-10, Apple Silicon, 3440×1440)

- 7 new unit tests pass; clippy/fmt clean.
- Visual: lawn density in grassland, sparser forest floor, dry desert tufts,
  clean riverbanks/coastlines, seamless near/far transition (no visible grass
  horizon), sway confirmed by frame differencing (~2.7 % of center pixels
  changed over 1 s with a static camera).
- Two benchmark gotchas surfaced on the way: the stock flythrough path flies
  mostly over open sea (useless for vegetation cost), and on a 100 Hz display
  the "non-vsync" benchmark can lock to the refresh rail, masking the real
  delta. `benchmarks/countryside.json` (added here) loops over the grassland /
  river region instead.
- Countryside A/B: no grass 82.4 avg FPS (12.13 ms mean) → with grass 77.6 avg
  (12.88 ms): **−5.8 % avg FPS, +0.75 ms/frame**. 1 % low 37.6 → 31.8 (grass
  tile generation overlapping chunk-gen spikes at 180 m/s benchmark flight;
  much milder at normal play speeds). `benchmarks/baseline.json` is stale
  (recorded in the 30.9 FPS era) and was deliberately left untouched.

## Risks / known simplifications

- **Tint at biome borders** uses the nearest classified biome (no two-biome
  blend like the terrain fragment); density fading toward non-grass biomes
  keeps the mismatch minor.
- **Bilinear vs. triangle-mesh height**: roots can be a few cm off on steep
  diagonal slopes; sinking roots 4 cm hides it (same trick flora relies on).
- **Perf levers** if grass ever needs to get cheaper: `SPACING` (0.5 → 0.65),
  `GRASS_FAR` (90 → 70), and `GRASS_LOD_DISTANCE` (40 → 25); all
  single-constant changes.
