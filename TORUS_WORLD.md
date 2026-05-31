# Finite Wrapping World (Torus) — Design

## Status

**Decision made.** We are converting the world from an unbounded flat plane to a
**finite, wrapping flat plane (a torus)**. This is a living design doc — sections
marked _OPEN_ are still under discussion.

**Scope of this change: the geometric foundation only.** The plant life-cycle
**simulation and ecosystem** work (sim scope, sustainability tuning, delta
compaction — sections #4–#6) is the *motivation* for going finite, but it is
**out of scope for this change** and deferred to a later one. This change makes the
world a torus geometrically; it does **not** alter how plants spread, die, or are
simulated. See [Out of scope](#out-of-scope-deferred-to-a-later-change).

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

## Out of scope (deferred to a later change)

The following are the *reason* we want a finite world, but they are **not part of
this change**. This change delivers the torus geometry only; the simulation keeps
behaving exactly as it does today (loaded-only spreading with capped catch-up).
These are captured here so we don't lose the design context.

### S1. Lifecycle sim scope — _DEFERRED_

A finite world **enables** a better sim but doesn't give it for free. Today
spreading ticks **only loaded chunks**, with capped catch-up on reload. With a
bounded chunk set there's a real choice to make *later*:

- **Keep loaded-only + catch-up.** Cheap. The biosphere is lazily evaluated —
  frozen until visited, then deterministically replayed. Returnability works but
  far regions don't truly "live" while away.
- **Two-tier sim (coarse-global + fine-local).** A low-frequency, low-resolution
  pass over **all `N²` chunks** (aggregate population/biome health per chunk, not
  per-plant) that runs regardless of what's loaded; refined into individual plants
  on arrival. This is what makes "the whole biosphere develops while you're away"
  actually true rather than approximated.

_Decide later: lazy vs. two-tier._

**Caveat that touches this change:** spreading already uses
`hash4(seed, cx, cz, i)`. Once the sim runs on a torus, those chunk ids must be the
**canonical (wrapped)** ids so a seed landing across the seam is deterministic.
That canonicalization is a *small* follow-up, but it is **not** done here — for now
the sim continues to use raw ids, which is harmless until the deferred sim work
begins. Flagged so it isn't forgotten.

### S2. Sustainability tuning — _DEFERRED_

A closed system needs equilibrium, or populations explode to fill every tile or
collapse to zero. To resolve later:

- Is there **mortality / death**? Per-plant lifespan?
- **Carrying capacity** per chunk or per biome (resource competition)?
- What's the target dynamic — stable equilibrium, or boom/bust cycles?

### S3. Persistence / DeltaStore growth — _DEFERRED_

Finiteness caps spatial extent but not **history**: a long sim could accumulate
unbounded deltas. Need periodic **compaction** of deltas into a fresh base state so
the store stays bounded over real time, not just over space.

## Suggested sequencing

**This change (geometric foundation only):**

1. Introduce `N` and a `canonical(raw_id) = raw_id.rem_euclid(N)` helper (#1, #4).
2. 4D-torus tileable noise in [heightmap.rs](src/world_core/heightmap.rs) (#2).
3. Route generation/persistence content lookups through `canonical()`; leave
   streaming, culling, and placement on raw ids (#1, #3).

That's the whole change — the world becomes a seamless torus. Streaming behaves as
today; the sim behaves as today.

**Later changes (deferred, see [Out of scope](#out-of-scope-deferred-to-a-later-change)):**

4. Canonicalize sim chunk ids so spreading is deterministic across the seam (S1).
5. Sim scope decision + implementation — lazy vs. two-tier (S1).
6. Sustainability tuning (S2) and delta compaction (S3).

## Open decisions checklist

**Geometric foundation (this change):**

- [x] **1a (wrap at lookup)** vs 1b — DECIDED: 1a, camera position grows unbounded
- [x] Noise: **4D-torus wrap** vs integer-period domain wrap — DECIDED: 4D-torus
- [x] Streaming across the seam — DECIDED: follows from 1a (raw ids for
  streaming/placement, canonical ids only at content lookup; no seam special-case)
- [x] World size `N` — DECIDED: `N = 256` (~65 km², ~65k chunks; `L = 65 536 m`)

**Deferred to a later change (out of scope here):**

- [ ] Sim scope: lazy (loaded-only) vs two-tier (coarse-global + fine-local)
- [ ] Mortality / carrying-capacity model for equilibrium
- [ ] Delta compaction strategy
- [ ] Canonicalize sim chunk ids for seam-correct, deterministic spreading
