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
3. A `foliage.style` string (`"needle"` / `"broadleaf"` / `"palm_frond"`) used to
   exist in the config and was set to `"needle"` for spruce, but it was only ever
   checked for `"none"` and palm routing — it had **zero effect on geometry**.
   (Phase 1, now landed, replaced this string with a typed `foliage.leaf_type:
   LeafType`; it still doesn't drive geometry yet — that's Phase 2.)

So the data hook was half-present; nothing consumes it for shape yet.

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
shrubs use the billboard pipeline. Therefore, the renderer needs to draw a
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

### Phase 2 — Split the plant mesh data model

- Keep visible output unchanged.
- Replace the single `TreeData { segments, foliage: Vec<FoliageBlob>, ... }`
  shape with a representation that can carry multiple foliage element kinds:
  broadleaf SDF blobs now, needle cards later.
- Keep `Broadleaf`, `PalmFrond`, and existing live trees on the current
  `FoliageBlob` → SDF path.
- Keep `Needle` temporarily mapped to the old blob path while the new data
  model lands, but isolate that mapping behind a clear dispatch point.
- Teach `PlantMesh` about named submeshes or mesh parts, even if Phase 2 still
  returns only one opaque part.
- Add focused tests/assertions for:
  - broadleaf output staying non-empty and opaque,
  - `LeafType::None` producing no foliage part,
  - `Needle` reaching a separate dispatch branch without changing visuals yet.

**Done means:** `cargo check` passes, the old broadleaf/spruce visuals are
unchanged, and the code has an obvious place to plug in needle card generation
without disturbing SDF broadleaf generation.

**Status:** complete

Done:

- Added typed foliage elements in `tree.rs`: broadleaf, palm, scale-leaf
  fallback, and needle fallback SDF blobs now carry their foliage kind instead
  of living in an undifferentiated `Vec<FoliageBlob>`.
- Kept `Needle` deliberately on the old SDF blob output for this phase, but
  routed it through `SdfFoliageKind::NeedleFallback` so Phase 3 can replace that
  branch with card generation.
- Added `PlantMeshPart` / `PlantMeshPartKind` metadata to `PlantMesh`. Existing
  renderer callers still consume the flattened `vertices` and `indices`, while
  Phase 4 can use the part list to split opaque/card draws.
- Added focused tests for broadleaf vs. needle dispatch, leafless dead trees,
  and the single opaque mesh part emitted during Phase 2.

### Phase 3 — Generate needle card mesh data

- Add a `NeedleCard`/foliage-card element with position, local axes or corners,
  size, tint variation, and a stable per-card random value.
- Route `LeafType::Needle` to emit card elements distributed along spruce branch
  tips instead of adding SDF blobs.
- Build fixed-orientation quad vertices from those elements, using the existing
  `Vertex` layout convention where the colour attribute can pack card UVs for
  alpha-tested foliage shaders.
- Preserve bark cylinder generation exactly as-is.
- Keep this phase CPU/data-only where possible: it can produce a card submesh,
  but it does not have to be drawn with the final shader yet.

**Done means:** spruce mesh generation produces a separate foliage-card mesh
part with sane vertex/index counts, broadleaf still uses SDF blobs, and dead
snags remain bark-only.

**Status:** not started

### Phase 4 — Draw mixed opaque/card plant submeshes

- Add a dedicated needle-card shader, sibling to `shrub_billboard.wgsl`, with a
  procedural needle-spray alpha mask — no textures.
- Extend the instanced renderer so one species can draw:
  - opaque bark/SDF submesh through the normal instanced pipeline,
  - alpha-tested card submesh through `create_billboard_pipeline`.
- Keep foliage cards out of the shadow pass initially. Bark continues to cast
  shadows.
- Reuse the same instance buffers for both submesh draws so plant placement,
  scale, tint, tilt, LOD, and dead-state handling stay unified.

**Done means:** spruce no longer renders as SDF globules in-world; bark and
needle cards render together for the same plant instances; shrubs still use
their existing billboard path.

**Status:** not started

### Phase 5 — Editor, screenshots, benchmark

- Expose `leaf_type` in the plant editor.
- Verify in-world via `take_screenshot` (debug CLI `screenshot`).
- Screenshot-loop tune spruce card size/count/mask against a real spruce in the
  world.
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
