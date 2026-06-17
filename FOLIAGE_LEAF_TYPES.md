# Foliage leaf types — needles vs. broadleaves

## Problem

Pine/conifer foliage currently renders as soft, bulbous "globules" — big bubbles
of mass — instead of needles. The cause is that **all** foliage, regardless of
species, is built identically:

1. Generation produces a list of `FoliageBlob { center, radius, … }` — literally
   spheres (`src/world_core/plant_gen/tree.rs`).
2. `extract_foliage_surface` (`src/world_core/plant_gen/sdf.rs`) packs those
   spheres into a signed-distance field, `smooth_min`-unions them, and
   surface-nets a mesh. Smooth-unioned spheres → blobby globules.
3. A `foliage.style` string (`"needle"` / `"broadleaf"` / `"palm_frond"`) already
   exists in the config (`src/world_core/plant_gen/config.rs`) and spruce is set
   to `"needle"` (`species/spruce.json`) — but it is only ever checked for
   `"none"` and palm routing. It has **zero effect on geometry**.

So the data hook is half-present; nothing consumes it for shape.

## Terminology

- **Broadleaf** — flat, wide leaf blades (oak, birch, willow). The blade is the
  botanical *lamina*; shape is *laminar*. These are the "regular leaves."
- **Needle / coniferous** — needles are *acicular* leaves (spruce, pine).
- Room for future types: **scale** (cypress, cedar), **frond** (palm), etc.

The new property is named **`leaf_type`**.

## Decisions (locked)

- **Needle rendering: billboard needle-spray cards.** Alpha-tested cards with a
  procedural needle-spray mask, building on the existing shrub billboard
  pipeline (`src/renderer_wgpu/instancing.rs` `shrub_billboard_mesh`,
  `shaders/shrub_billboard.wgsl`, `pipeline.rs` `create_billboard_pipeline`).
  This is the "simpler shader" path.
- **Broadleaf scope: unchanged.** Oak/birch/willow keep the current
  `FoliageBlob` → SDF path. The `leaf_type` abstraction is still built so
  broadleaf can be reworked later.
- **Shadows:** foliage cards skip shadow-casting initially (shrub cards already
  do, to avoid rectangular shadow artifacts — `instanced_pass.rs`). Bark still
  casts.
- **Card orientation:** fixed-orientation cards baked into the mesh, drooping
  along branches — *not* camera-facing billboards. Matches the shrub approach.
- **No `gen_key` bump needed.** `leaf_type` is a *rendering* trait. The
  `generation_key` (`src/world_core/herbarium.rs`) deliberately hashes only
  base-generation inputs — `BaseGenerationPlant` carries name/kind/height/
  placement, not foliage — and the `generation_key_ignores_plant_rendering_traits`
  test enforces it. Plant meshes regenerate at runtime from `SpeciesConfig`, so
  foliage changes never touch `world_base.bin`. (No-compat for saves still holds
  if a later phase ever needs it, but foliage work doesn't.)

## Architecture note

A needle conifer is **opaque bark geometry + alpha-tested foliage cards** within
the *same* species. Today a tree mesh is entirely opaque (one pipeline) and only
shrubs use the billboard pipeline. Phase 3 therefore needs the renderer to draw a
single species across **two pipelines** — an opaque bark submesh + an
alpha-tested foliage-card submesh. The instanced pass already switches pipelines
per-species for shrubs (`instanced_pass.rs`); extend that to a per-submesh split
rather than a per-species one.

## Phases

### Phase 1 — `leaf_type` as a first-class property

- Add a `leaf_type` field to `Foliage` (`src/world_core/plant_gen/config.rs`) —
  an enum (`Broadleaf`, `Needle`, future `Scale` / `Frond`). Fold the real
  meanings of the loose `style` string into the typed enum; keep a coverage/no
  foliage flag for the `"none"` case.
- Thread it through:
  - species JSON (`species/*.json`) — spruce → `needle`, others → `broadleaf`.
  - `PlantParams` + the plant editor merge (`src/app/plant_editor.rs`).
- No visual change yet. This is the data spine.

**Status:** ✅ complete

Done:

- Replaced `Foliage.style: String` with `Foliage.leaf_type: LeafType`, a typed
  enum (`None`, `Broadleaf`, `Needle`, `ScaleLeaf`, `PalmFrond`) that serializes
  to the same snake_case labels the JSON and editor already used. Added
  `has_foliage()`, `label()`, and `from_label()` helpers
  (`src/world_core/plant_gen/config.rs`).
- Migrated all callers: the `"none"` / `"palm_frond"` string checks in
  `tree.rs`, `deadify()` in `config.rs`, the plant-editor merge
  (`src/app/plant_editor.rs`), and `PlantParams::from_species`
  (`src/ui/plant_editor_panel.rs`). The editor still carries a `foliage_style:
  String` at the UI layer and converts at the `SpeciesConfig` boundary.
- Renamed the `"style"` key → `"leaf_type"` in all 8 species presets (values
  unchanged).
- No `gen_key` bump (see Decisions). `cargo check`, `clippy`, and the full lib
  test suite (115 tests) pass.

### Phase 2 — Dispatch foliage generation on `leaf_type`

- `generate_plant_mesh` (`src/world_core/plant_gen/mod.rs`) branches on
  `leaf_type`:
  - `Broadleaf` → existing `FoliageBlob` → SDF path (unchanged).
  - `Needle` → new path emitting **needle-spray card placements** (position,
    orientation along branch, size, tint) instead of spheres.
- Introduce a foliage-element abstraction (enum/trait) rather than a single
  `Vec<FoliageBlob>`, so future leaf types slot in cleanly.

**Status:** not started

### Phase 3 — Needle-spray card geometry + shader

- Generate small fixed-orientation quad cards distributed along branch tips,
  oriented to droop with the branch (reuse the spruce `droop` / `coverage` /
  `whorled` structure already in the preset).
- Render via the existing alpha-tested billboard pipeline
  (`pipeline.rs` `create_billboard_pipeline`) with a **procedural needle-spray
  alpha mask** in a dedicated shader (sibling to `shrub_billboard.wgsl`) — no
  textures, consistent with the current art pipeline.
- Extend the instanced pass to draw the opaque bark submesh + alpha-tested
  foliage-card submesh per species (see architecture note).
- Screenshot-loop tuning against a real spruce in-world.

**Status:** not started

### Phase 4 — Editor, verification, benchmark

- Expose `leaf_type` in the plant editor.
- Verify in-world via `take_screenshot` (debug CLI `screenshot`).
- Run the FPS benchmark — card overdraw will shift vert counts
  (`bun tools/bench.ts`).

**Status:** not started

## Key files

| Concern | File |
| --- | --- |
| Foliage config struct | `src/world_core/plant_gen/config.rs` |
| Sphere blob data + generation | `src/world_core/plant_gen/tree.rs` |
| SDF surface extraction (globules) | `src/world_core/plant_gen/sdf.rs` |
| Mesh assembly | `src/renderer_wgpu/mesh.rs` |
| Plant mesh entry point | `src/world_core/plant_gen/mod.rs` |
| Spruce preset | `src/world_core/plant_gen/species/spruce.json` |
| Billboard mesh + pipeline | `src/renderer_wgpu/instancing.rs`, `pipeline.rs` |
| Billboard shader | `src/renderer_wgpu/shaders/shrub_billboard.wgsl` |
| Instanced draw / pipeline switching | `src/renderer_wgpu/instanced_pass.rs` |
| Plant editor merge | `src/app/plant_editor.rs` |
