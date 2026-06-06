# Architecture review — improvement backlog

A snapshot review of the overall architecture (2026-06-06), captured here so we
can work through the findings later. Nothing below is urgent: the codebase is in
good shape. This is a "sharpen a good thing" list, ordered by impact-to-effort.

## What's already healthy (don't break it)

The three-layer split is **actually enforced**, not just aspirational:

- `world_core/` imports zero `wgpu`, `egui`, `renderer`, `app`, or `ui` — pure
  domain logic (grep-verified).
- `world_runtime/` imports zero rendering/GPU concerns — only `std`, `glam`,
  `rayon`, `world_core`.

That clean directional dependency graph is the hardest thing to get right and the
easiest to erode. Keep it intact. Error handling is also disciplined: 0 unwraps in
`ui`/`audio`, only 2 `panic!`s total, no `todo!`.

## The improvements that matter

### 1. Split the snapshot codec out of `world_runtime/plant_world.rs` (1,981 lines)

Clearest structural win. The file conflates two unrelated things:

- **Simulation** (growth/spread ticks, spatial grid, population state) — belongs here.
- **A bespoke binary format** — `serialize_base`/`from_base_snapshot` plus Brotli,
  varint, and columnar-encoding helpers (~280 lines + ~450 lines of tests for it).
  This is an *encoding concern*, not runtime orchestration.

Extract the snapshot codec to `world_core/persistence/plant_snapshot.rs`. That
alone roughly halves the file and makes the format swappable/testable in isolation.

- `serialize_base` / `from_base_snapshot`: `src/world_runtime/plant_world.rs:548-820`
- helpers (`write_varint`/`read_varint`, `brotli_compress`/`decompress`): `:1303-1365`

Keep `save_spread` / `apply_saved_spread` where they are (thin storage wrappers).

### 2. Encode the `Screen`/`world` invariant in the type system (`app::AppState`)

`AppState` has ~50 fields (`src/app/mod.rs:88`). Most of that bloat is inherent to
a centralized event loop — not worth fighting. But one part is genuinely
un-idiomatic: `world: Option<WorldRuntime>` paired with a `Screen` enum, where
"world exists iff Playing/Editor" is enforced by hand with
`self.world.as_mut().unwrap()` scattered across the code:

- `src/app/debug_commands.rs:24`, `:134`, `:208`, `:403`
- `src/app/mod.rs:1179`, `:1242`

The idiomatic move is to make `Screen` own the state each screen needs:

```rust
enum Screen {
    StartMenu,
    Loading(LoadingState),
    Playing { world: WorldRuntime },
    // ...
}
```

This makes invalid states unrepresentable and deletes every `.unwrap()` on `world`.
Highest-leverage idiom improvement in the codebase, but it touches more files —
worth a dedicated PR.

Secondary (cosmetic, low value): the screenshot cluster (`screenshot_pending`,
`screenshot_to_clipboard`, `screenshot_toast`, `captures_dir`) and the loading-map
cluster (`loading_map_tex/buf/done`) could each become a small sub-struct.

### 3. Close the renderer's leak of a `world_core` concern

`instanced_pass.rs` calls `plant_gen::generate_plant_mesh()` directly
(`src/renderer_wgpu/instanced_pass.rs:245`, `:262`). Mesh *generation* is domain
logic; the renderer should receive vertices, not generate them. This is the one
place the otherwise-clean layering bleeds. Move generation up to `world_runtime`
(or a prep step) and hand the renderer pre-built meshes.

### 4. De-duplicate the placement grids in `world_core/content/flora.rs`

`place_grid` and `place_aquatic_grid` share ~70% identical scaffolding (jitter/RNG,
eligible-species filter, weighted selection, height/rotation), duplicated at
`src/world_core/content/flora.rs:92-218` vs `:265-368`. A shared `place_on_grid()`
helper parameterized by the land-vs-aquatic predicate removes a real
two-places-to-fix-bugs hazard. Also give the magic seed offsets
(`0, 2000, 5000, 71, 193…`) names.

## Smaller things / judgment calls

- **No `Pass` trait in the renderer.** Each `*_pass.rs` is ad-hoc and
  `WorldRenderer::render_scene` wires them manually. Recommend **leaving this** —
  `pipeline.rs` and `material.rs` already factor the real boilerplate, and the
  passes are heterogeneous enough (different sync signatures; some have no sync)
  that a forced trait would be abstraction for its own sake. One cheap win: the
  identical quad-grid index-buffer generation is copy-pasted in 3 spots
  (`terrain_compute.rs:118`, `water_pass.rs`, `river_pass.rs`) → one helper.
- **The `terrain.rs` / `terrain_fields.rs` / `heightmap.rs` trio** reads as tangled
  by name, but responsibilities are distinct (layer / baked low-freq grids /
  sampling API). `heightmap.rs` (470 lines) mixes per-vertex sampling with
  per-chunk grid building — a candidate split, but defensible. Naming clarity
  matters more here than restructuring.
- **`herbarium.rs` (532 lines)** does config-loading + dedup + `PlantRegistry` +
  gen-key hashing. `PlantRegistry` could be its own module, but it's coherent. Low
  priority.
- **`ui/plant_editor_panel.rs` (1,053 lines)** is big but pure UI with clean data
  flow (no world access). Split into crown/trunk/foliage sections if it keeps
  growing; not urgent.
- **Config is not duplicated** despite four `config.rs`-ish files — they're
  genuinely distinct domains (game rules / debug server / plant genetics / UI
  rendering). No action.

## Suggested first three

1. **Split the snapshot codec out of `plant_world.rs`** → `world_core/persistence/`.
   Biggest file, cleanest cut, self-contained.
2. **De-dup `flora.rs` placement grids** → small, removes a real maintenance trap.
3. **Fold `world` into the `Screen` enum** → deletes the runtime `unwrap`
   invariant; most idiomatic-Rust gain, but touches more files (dedicated PR).
