# Finite Wrapping World (Torus) — Design

## Status

**Geometric foundation: shipped** (#55). The world is now a finite, seamlessly
wrapping flat plane (a torus) — `256 × 256` chunks, periodic 4D-torus noise,
canonical chunk ids at content lookup. The sections below on coordinate wrapping,
tileable noise, streaming, and world size are **done** and kept as the design
record.

**Now active: the live simulation.** The plant life-cycle simulation was the
*motivation* for going finite and was deferred from the geometry change. Its
direction is now decided — see [Live simulation](#live-simulation-the-deferred-work).
After exploring aggregate vs. per-plant models, we chose to **simulate every plant
in every chunk individually**, globally, from `t=0`, because the long-term goal is
to play with **evolution** (per-plant heritable rules), and a per-individual
substrate is the natural — and aggregation-hostile-to-evolution — foundation.
**Iteration 1** (specified below) runs that global per-plant sim with **constant
traits** and **grow-to-capacity** dynamics; death and genomes follow in later
iterations.

## Why (the requirement)

The world is generated procedurally, but it then runs **plant life-cycle
simulation**: plants sprout, grow through stages, spread via random factors, and
(should) die out and recover. An indefinitely large world (tiles created on
demand at the horizon) is incompatible with this, because:

1. **Population is unbounded** — there's no cap on the number of plants the sim
   must progress.
2. **Returnability requires a finite, persistent world** — go back to a place and
   see how its biosphere evolved.
3. **A sustainable closed system** — spread, death, and recovery only make sense
   in a bounded ecosystem.

A **sphere** would satisfy this, but a true visible-curvature planet is a renderer
rewrite (cube-sphere chunking, 3D noise sampling, and — the killer — `f64` world
coordinates + floating-origin rendering to survive an Earth-radius sphere in
`f32`). A **torus** delivers all three goals while staying flat and reusing the
existing renderer, deterministic generation, persistence, and chunk-keyed sim.

Locally the torus is indistinguishable from today's flat world; globally it is
finite and wraps seamlessly — walk far enough in one direction and you return to
where you started. "Like Earth" in _behavior_, without the planet-rendering cost.

## What we are NOT doing

- No visible curvature / round-from-orbit planet.
- No `f64` / floating-origin rewrite (not needed for a bounded flat world).
- No cube-sphere or lat/lon chunking; the integer chunk grid stays.

If we ever want a visible sphere, that's a separate project with its own decision.

## Core model

- The world is **`N × N` chunks** with **`N = 256`** (decided — see #4). Chunk
  coordinates wrap: `chunk(x, z) ≡ (x mod N, z mod N)`.
- Chunk size stays **256 m**, grid resolution **129×129** (unchanged). World side
  `L = N · 256 m = 65 536 m` (~65 km).
- `N` is a single named constant (alongside `CHUNK_SIZE_METERS`); it's part of the
  world format, since changing it invalidates existing saves.
- There are only `N²` **distinct** chunks ever (~65,536). This bounds:
  - the plant population the sim must progress,
  - the persistent delta store's spatial extent.

## Current architecture (what we're changing)

| Concern | File | Today |
|---|---|---|
| Chunk coords | [chunk.rs](src/world_core/chunk.rs) | `IVec2`, unbounded; `floor(world_pos / 256)` |
| Streaming | [streaming.rs](src/world_runtime/streaming.rs) | load radius square around camera; unloaded chunks discarded |
| Heightmap | [heightmap.rs](src/world_core/heightmap.rs) | 3-layer OpenSimplex, `(x,z) → height`, non-periodic |
| Lifecycle sim | [lifecycle_sim.rs](src/world_runtime/lifecycle_sim.rs) | spreads only for loaded chunks; deterministic `hash4(seed, cx, cz, i)` |
| Persistence | delta_store.rs | tracks all non-base plants, saved to disk |
| Coords/precision | [camera.rs](src/renderer_wgpu/camera.rs) | `Vec3` / `f32` |

## Implementation areas (to discuss & sequence)

Each of these is a discussion point — we'll decide the approach per area before
writing code.

### 1. Coordinate wrapping — **DECIDED: 1a (wrap only at lookup)**

We use a single canonical "world → chunk" mapping that wraps. **The camera's
`f32` position grows unbounded as today**; we apply `rem_euclid(N)` only when
converting a world position to a *canonical* chunk id for generation, sim, and
persistence. Rendering still uses the raw position, so chunks are placed at the
camera's actual location and the seam is invisible.

- Pro: no camera teleport, no discontinuity. Simplest rendering. Reuses existing
  camera/streaming/culling unchanged except for the id mapping.
- Accepted con: `f32` position grows without bound on very long flights →
  precision drift can eventually return. We accept this for now; if it becomes a
  real problem we can revisit (recenter, or move to 1b).

Concretely: a single helper that turns any world `(x, z)` (or raw chunk id) into a
**canonical chunk id** via `rem_euclid(N)`. Everywhere a chunk id feeds generation,
the lifecycle sim, or persistence, it must use the canonical id. Streaming and
rendering keep using the *raw* (unwrapped) chunk id for placement so the world
appears continuous; only the **content lookup** is wrapped.

Implication to watch: two raw chunk ids that are `N` apart map to the **same
canonical chunk**, sharing terrain *and* persisted plant state — that's the point.
A single canonical chunk only appears at two raw positions *at once* if the load
region spans a full lap of the world, i.e. `2·radius + 1 ≥ N`. With the default
radius 3 (diameter 7) and any reasonable `N` (≥ 128), this never happens, so the
renderer never sees a duplicate canonical chunk in one frame. We just need `N`
comfortably larger than the load diameter — easily satisfied. _Confirm in #3._

### 2. Tileable (periodic) noise — **DECIDED: 4D-torus wrap**

The heightmap and moisture noise must be **periodic with period `L = N·256 m`** so
the terrain matches across the wrap seam (no cliff where east meets west). Today's
`OpenSimplex(x·freq, z·freq)` is not periodic.

We sample noise on a **torus embedded in 4D**: map each world axis to a circle and
feed the four circle coordinates to a 4D noise function. The field is then exactly
periodic in both axes with no visible distortion, and — crucially — **frequencies
stay continuous** (no need to quantize them to integer periods, which is the
drawback of plain domain-wrapping).

**Mapping.** For a layer with spatial frequency `f` (cycles per metre) over a world
of side `L` metres, the number of whole noise periods around the loop must be an
**integer** `k` so the loop closes seamlessly:

```
k     = round(f · L)              // whole noise cycles around the world, per axis
r     = k / (2π)                  // circle radius that yields k cycles per lap
θx    = 2π · (x mod L) / L
θz    = 2π · (z mod L) / L
noise4(r·cos θx, r·sin θx, r·cos θz, r·sin θz)
```

So the only quantity we snap to an integer is `k` (cheap, per-layer, done once),
not the frequency the rest of the code reasons about. Effective frequency becomes
`k / L`, a hair off the requested `f` — negligible at these `k`.

- Use the `noise` crate's 4D OpenSimplex (`get([f64; 4])`).
- Apply to **all** layers: continental, ridge, detail, and both moisture layers.
  Each keeps its own seed offset (+101, +907, etc.) and its own `k`.
- The three height layers and two moisture layers all share the same `L`, so the
  composite field is periodic.
- Centralize the `(x, z, f) → [f64; 4]` mapping in one helper in
  [heightmap.rs](src/world_core/heightmap.rs) so every layer wraps identically.

_Watch:_ low-frequency layers have small `k` (e.g. continental at `L = 65 536 m`
→ `k` in the dozens). That's plenty of cycles; no visible stretching. If any layer
rounded to `k = 0` or `1` we'd have a problem, but none do at sane `N`. (Exact
per-layer frequencies live in [heightmap.rs](src/world_core/heightmap.rs).)

**Big simplification — noise lives in exactly ONE place.** Investigation confirms
the GPU does **not** generate noise: [heightmap.rs](src/world_core/heightmap.rs) is
the sole noise implementation. The CPU computes per-chunk height/moisture arrays in
[terrain.rs](src/world_core/terrain.rs), and:

- the **terrain compute shader** ([terrain_gen.wgsl](src/renderer_wgpu/shaders/terrain_gen.wgsl))
  receives those arrays as read-only storage buffers — it only meshes, computes
  normals, and colours; it has no noise functions;
- **plant placement** ([flora.rs](src/world_core/content/flora.rs)) samples the same
  CPU arrays bilinearly, so plants automatically agree with the rendered terrain.

So the 4D-torus wrap is a change to **one function family in one file**. No WGSL
port, no CPU/GPU consistency risk. Biome classification is duplicated (CPU
[biome.rs](src/world_core/biome.rs) and the shader) but takes height/moisture as
*inputs* — it doesn't compute noise, so it's unaffected.

### 3. Streaming across the seam — **DECIDED (follows from 1a)**

With 1a settled, this is mechanically determined — no separate choice to make.

Streaming continues to work in **raw (unwrapped) chunk ids**: the load radius
square is computed around the camera's raw chunk exactly as today, and each loaded
chunk is placed at its raw position. Because the camera position is unbounded and
rendering uses it directly, the world is visually continuous and there is no seam
to special-case in the streaming/culling path.

The **only** change is at content fetch time: when a raw chunk needs its terrain or
plant state, we look it up by its **canonical id** (`raw.rem_euclid(N)`). So raw
chunks `5` and `5 + N` transparently resolve to the same generated terrain and the
same persisted plant state — the wrap is invisible to streaming, frustum culling,
and placement, all of which keep operating on raw ids.

Confirmed from #1: a single canonical chunk can't appear at two raw positions in
one frame unless the load region spans a full lap (`2·radius + 1 ≥ N`), which never
happens for sane `N`. So no duplicate-chunk handling is needed.

Net effect: streaming/culling code is essentially **unchanged**; only the
generation/persistence lookups get the `canonical()` call. This collapses into the
same one-line mapping as #1.

### 4. World size `N` — **DECIDED: `N = 256`**

The world is **256 × 256 chunks** → side `L = 256 · 256 m = 65 536 m` (~65 km) →
~65,536 distinct chunks total.

Why 256: a comfortable middle ground — large enough that the wrap is rarely
noticed in normal flight, small enough to persist cheaply, and (with the ecosystem
work deferred) **unconstrained by simulation cost** for this change. Easily clears
the only hard requirement, `N ≫` load diameter (7). For reference, the rejected
options:

- `N = 128` → ~33 km², ~16k chunks (wrap noticeable sooner)
- **`N = 256` → ~65 km², ~65k chunks (chosen)**
- `N = 512` → ~131 km², ~262k chunks (heavier to persist/simulate later)

We can revisit if the deferred sim work wants a different size — but note changing
`N` invalidates existing saves (see below), so it's best treated as fixed.

One forward-looking note: `N` should be defined as a single named constant
(alongside `CHUNK_SIZE_METERS`) so the future sim work and any save-format
versioning can reference it. Changing `N` later invalidates existing saves, so
treat it as part of the world seed/format.

## Live simulation (the deferred work)

The finite world exists so the plant life-cycle can be a **real, closed,
persistent ecosystem** rather than the loaded-only approximation we ship today.
The long-term goal is to play with **evolution**: each plant carries heritable
rules (a genome) that mutate on reproduction and are shaped by selection over long
time. That goal is what fixes the architecture.

### Why per-plant, not aggregate

We explored a two-tier model (a coarse per-chunk aggregate as the persistent truth,
concrete plants realized on load). It is cheap and bounded, but it is **hostile to
evolution**: a genome *distribution* cannot be compressed to a per-species scalar
count — averaging destroys exactly the within-population variation that selection
acts on. So the substrate must be **per-individual**. The existing code is already
a decent evolutionary substrate: `validate_seedling_landing` (spacing / biome /
sea-level checks) is effectively a fitness function selecting which seeds survive.

### Decided shape

- **Simulate every plant in every chunk, individually.** No coarse/fine split.
- **Global from `t=0`.** All `N²` (65,536) canonical chunks tick on one global
  clock; the whole world lives, loaded or not.
- **All plants** (trees *and* shrubs) are in the sim.
- Iteration 1 keeps **constant traits** (today's species config) — no genome yet —
  and **grow-to-capacity** dynamics — no death yet. Death and genomes are
  iterations 2 and 3, with seams left for them.

### Feasibility (the numbers we sized against)

Per chunk: ~529 tree slots (11 m grid) + ~4096 shrub slots (4 m grid), filled at
biome density (Forest up to 0.72). A dense forest chunk ≈ 3,300 plants (~90 %
shrubs). Across 65,536 chunks: **~25–65 M plants**, peaking somewhat higher once
spread densifies past base biome density.

- **RAM:** ~40 M plants × ~32 B packed ≈ **~1.3–2 GB resident**. This is the
  binding constraint; the `Plant` struct must be packed (chunk-local quantized
  position, `u8` species/rotation/stage), not today's `Vec3`+`usize` layout. Big
  later lever if RAM bites: shrubs as **genets** (one sim entity → many billboards),
  cutting evolving count 5–20×.
- **Startup (first world creation only):** generating terrain+flora for all 65k
  chunks is tens of seconds on 16 cores — paid **once**, then persisted and mmap'd
  on later loads. Not a per-session cost.
- **Tick:** growth is **analytic** (computed from `born_hour`, ~free). The periodic
  global **spread** pass scans mature plants — parallel via rayon, ~100–300 ms,
  decoupled from frame rate and time-sliced. Cost scales with churn, not raw count.

### Iteration 1 — global per-plant sim, fill-to-capacity

> **Build doc:** the implementation sequencing (milestones, current-state map,
> acceptance checks) lives in
> [docs/LIVE_SIM_ITERATION_1.md](docs/LIVE_SIM_ITERATION_1.md). The summary below
> is the design intent.

**Goal:** every chunk's plants simulate continuously on one global clock from
`t=0`; sparse areas fill in via spread until spacing limits stop them; state
persists across sessions. No death, no genome. Notably this **removes** machinery —
the loaded-only catch-up scaffolding exists only to fake unsimulated chunks.

1. **Canonical sim domain + delete catch-up.** Operate over the 256×256 canonical
   grid; canonicalize spread targets (`world_to_chunk → canonical_chunk`). Delete
   `last_sim_hour`, `MAX_CATCH_UP_HOURS`, and the clamp/replay logic in
   [lifecycle_sim.rs](src/world_runtime/lifecycle_sim.rs). *Net code removed.*
2. **`PlantWorld` — resident full-world plant store.** Live plant list per
   canonical chunk, all 65,536 resident. Base must be resident anyway (spreaders
   include mature base plants; regenerating 65k chunks of flora per tick is far too
   expensive). Init = batch-generate base flora for the whole world once at
   creation (parallel; terrain generated transiently, then discarded).
3. **Global tick driver.** Replace `tick_loaded_chunk_growth`
   ([runtime.rs](src/world_runtime/runtime.rs)) with a global pass over all
   canonical chunks. Spread runs parallel at a fixed cadence, **decoupled from
   frame rate** and time-sliced so a pass never stalls a frame. Spacing-based
   landing validation is the capacity gate — confirm it genuinely terminates growth
   so the world settles instead of creeping forever.
4. **Render bridge.** Loaded chunks read plants from `PlantWorld` instead of
   regenerating base+delta; terrain still regenerated per loaded chunk for the mesh.
   Re-upload a chunk's instance buffer when spread changes its list.
5. **Binary paged persistence.** Replace the single pretty-JSON `deltas` blob
   ([delta_store.rs](src/world_runtime/delta_store.rs)) with binary region files
   (mmap load, write-back dirty regions). Represent as delta-from-base for now
   (constant traits → base is regenerable, so disk stays small); migrate to full
   lists when evolution removes base-regenerability.
6. **Telemetry + bench.** World population, per-biome fill %, spread events/tick,
   tick ms, resident MB. Bench the global pass; assert population converges to a
   bounded capacity.

**Risks to handle in iteration 1:**

- **Cross-chunk spread is a parallel write hazard.** A chunk's seeds land in
  *neighbor* chunks' lists. Parallelizing spread over chunks needs a two-phase
  gather/scatter: phase 1 each chunk emits `(target_canonical_chunk, seedling)`;
  phase 2 a merge step appends. Keeps it deterministic and race-free.
- **Spread cadence vs `day_speed`.** High day-speed → many spread passes/sec → CPU
  spikes. Cap passes-per-frame or run the sim on its own fixed timestep independent
  of render.

### Later iterations (seams left, not built yet)

- **Iteration 2 — death + carrying capacity.** Constant per-species lifespan; spread
  gated on capacity. Turns the static "full" state into a **living equilibrium**
  (sprout → mature → spread → die → regrow). This is the stable-equilibrium dynamic
  we're targeting; grow-to-capacity in iteration 1 is its precursor.
- **Iteration 3 — evolution.** Add a genome to the `Plant` struct; express traits
  (growth rate, mature height, spread radius/chance, tolerances, lifespan, seed
  count) from genes instead of fixed species config; mutate on reproduction;
  landing validation = selection. Species become *starting genomes*, not hard rules.
  Persistence migrates base→full lists (mutated lineages aren't seed-regenerable).
- **Shrub genets** — model shrubs as genetic clumps to cut evolving-individual count
  5–20×. Pull in whenever RAM becomes the constraint.

## Decisions log

**Geometric foundation (shipped, #55):**

- [x] **1a (wrap at lookup)** vs 1b — DECIDED: 1a, camera position grows unbounded
- [x] Noise: **4D-torus wrap** vs integer-period domain wrap — DECIDED: 4D-torus
- [x] Streaming across the seam — DECIDED: follows from 1a (raw ids for
  streaming/placement, canonical ids only at content lookup; no seam special-case)
- [x] World size `N` — DECIDED: `N = 256` (~65 km², ~65k chunks; `L = 65 536 m`)

**Live simulation:**

- [x] Sim model: aggregate (two-tier) vs **per-plant individual** — DECIDED:
  per-plant (evolution needs per-individual variation)
- [x] Scope: loaded-only vs **global from `t=0`** — DECIDED: global, all `N²` chunks
- [x] Coverage: **all plants** (trees + shrubs) in iteration 1
- [x] Iteration 1 dynamics: **grow-to-capacity, no death**; constant traits, no genome
- [ ] Iteration 2: mortality model + carrying-capacity tuning (stable equilibrium)
- [ ] Iteration 3: genome representation, trait expression, mutation/selection
- [ ] Persistence: binary region-file/mmap format details; delta-from-base →
  full-list migration trigger
- [ ] Shrub genets — if/when RAM forces it
