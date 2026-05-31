# Live Simulation — Iteration 1 Build Doc

The **how** for iteration 1. The **why/what** lives in
[TORUS_WORLD.md](../TORUS_WORLD.md) → "Live simulation". Read that first; this doc
assumes those decisions and just sequences the work.

**Goal:** every plant in every chunk simulates on one global clock from `t=0`;
sparse areas fill in via spread until spacing limits stop them; state persists.
**No death, no genome, constant traits.**

Keep it simple: reuse the existing per-chunk sim primitives, change the *driver*
from loaded-only to global, and don't rewrite persistence until it actually hurts.

## Current state (what we're changing)

| Concern | Where | Today |
|---|---|---|
| Sim driver | [runtime.rs](../src/world_runtime/runtime.rs) `update()` → `tick_loaded_chunk_growth` | ticks **only loaded** chunks |
| Per-chunk tick | [lifecycle_sim.rs](../src/world_runtime/lifecycle_sim.rs) `tick_chunk_lifecycle` | growth + spread, **raw** coords, per-chunk catch-up clock |
| Plant state | [chunk.rs](../src/world_core/chunk.rs) `ChunkContent { base_plants, plants }` | only for **loaded** chunks |
| Overlay/persistence | [delta_store.rs](../src/world_runtime/delta_store.rs) | global `HashMap`, pretty **JSON** |
| Render bridge | [streaming.rs](../src/world_runtime/streaming.rs) `reassemble_loaded_chunk` | assembles `base + delta` → uploads |

Growth is already **analytic** (stage computed from `born_hour` in
[lifecycle.rs](../src/world_core/lifecycle.rs)) — no per-tick growth work. The
per-chunk catch-up machinery (`last_sim_hour`, `MAX_CATCH_UP_HOURS`) exists *only*
to fake unsimulated chunks; once the sim is global it is deleted.

## Milestones

Each is an independently shippable PR with a concrete "done" check. Do them in
order — M0 is a go/no-go gate; catch-up can only be deleted after the sim is global
(M3), so don't reorder.

### M0 — Feasibility spike (throwaway)

The one thing to prove before building: can we hold and generate the whole world?

- Headless bench: generate **base flora for all 65,536 canonical chunks**, count
  plants, measure peak RAM and wall time (use the existing parallel generator;
  discard terrain after placing flora).
- Pin the packed `Plant` struct from the real plant count. Starting proposal
  (~16–20 B): local `x,z` as `u16` quantized over 256 m (~3.9 mm), `height` `u16`,
  `rotation` `u8`, `species` `u8`, `stage` `u8`, `born_hour` as `f32` hours.

**Done:** numbers in hand; struct sized; explicit go/no-go on the ~1.3–2 GB budget.
Throwaway code — not merged into the runtime.

### M1 — Canonicalize sim + delta ids

Safe in today's loaded-only world; makes a full lap returnable; survives into the
global model. Smallest real change.

- Key `DeltaStore` by `canonical_chunk(coord)`; canonicalize spread targets
  (`world_to_chunk` result) and the spread hashes in `lifecycle_sim.rs`.
- `reassemble_loaded_chunk` / `apply_chunk_delta` look up deltas by canonical id.

**Done:** fly one world lap (256 chunks) and the simulated changes are still there;
existing sim tests pass (updated for canonical keys).

### M2 — `PlantWorld` + global growth tick

The big architectural step. Resident, all-chunk plant store; render reads from it.
Growth only — no spread yet, so behavior is unchanged but the data path is new.

- New `PlantWorld`: live `Vec<Plant>` per canonical chunk, all 65,536. Init once at
  world creation via the parallel generator (base flora for the whole world).
- `runtime.update()` ticks `PlantWorld` on the global clock (growth is analytic, so
  this is cheap). Rate-limit to the sim cadence — **not** every frame; keep it on
  the main thread for now (no extra threading).
- Render bridge: loaded chunks read their plant list from `PlantWorld` instead of
  regenerating `base + delta`. Terrain still regenerated per loaded chunk.

**Done:** world boots with all chunks populated in `PlantWorld`; loaded chunks
render identically to before; telemetry shows total world plant count.

### M3 — Global spread + capacity; delete catch-up

Now the world actually fills in.

- Move spread into the global tick over all canonical chunks. **Two-phase** to stay
  race-free: phase 1 (parallel) each chunk emits `(target_canonical_chunk,
  seedling)`; phase 2 (serial merge) appends into `PlantWorld`.
- Keep today's cadence (spread every 24 sim-hours). Spacing-based landing
  validation is the capacity gate — verify spread **terminates** (population stops
  growing once full).
- Delete the loaded-only catch-up path now that every chunk ticks every step.

**Done:** start sparse, watch a region fill to a stable full state and stop;
catch-up code gone; existing tests green or removed with the dead path.

### M4 — Persistence + telemetry + bench

- Telemetry: world population, per-biome fill %, spread events/tick, tick ms,
  resident MB.
- Persistence: **keep the existing JSON store first** and measure save/load time
  and size against the full-world state. Only if it's unacceptable, swap to the
  simplest binary option — `bincode` per-region file (e.g. 32×32 chunks/region),
  write back dirty regions. Represent as delta-from-base (constant traits → base is
  regenerable) to keep size down.
- Bench the global tick (`bun tools/bench.ts`); assert population is bounded.

**Done:** save/reload round-trips the full world; tick cost measured and acceptable.

## Risks (handle as they arise)

- **Cross-chunk spread race** — solved by the M3 two-phase split; don't let parallel
  chunks write into each other's lists directly.
- **Spread cadence vs `day_speed`** — at high day-speed the global pass can run many
  times/sec. Cap passes-per-frame (run at most one global spread pass per frame and
  let sim-time lag if needed). Defer a dedicated sim thread.
- **Startup gen time** — full-world gen at creation is the new cost. Pay it once,
  then persist; on later loads read the saved state, don't regenerate.

## Out of scope (later iterations)

Death / carrying-capacity turnover (iter 2). Genome / mutation / selection
(iter 3). Binary persistence *unless* M4 shows JSON is inadequate. Shrub genets
(only if RAM forces it).
