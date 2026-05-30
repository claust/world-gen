# Performance Improvement Plan

> **Updated 2026-05-30** after building the deterministic FPS benchmark
> (`tools/bench.ts`, `src/app/benchmark.rs`) and a baseline. This revision
> replaces the earlier *estimated* priorities with **measured** ones. Every
> number below comes from the `flythrough.json` benchmark on the dev machine
> (Apple Silicon), attributing cost by gating individual passes / tuning
> parameters via temporary env vars and re-running the benchmark.

## Baseline (current state)

`benchmarks/baseline.json`, default flythrough, 1200 frames:

| Metric | Value |
|---|---|
| avg FPS | ~30 |
| 1% low FPS | ~16 |
| mean frame time | 32 ms |
| p95 / p99 frame time | 51 / 60 ms |
| **gpu_bound_ratio** | **0.98** |
| mean CPU-active time | **0.49 ms** |
| mean GPU-wait time | 31 ms |

### The single most important fact: we are ~98% GPU-bound

CPU-side work is **0.49 ms/frame** against **31 ms** of GPU wait. This
invalidates the framing of the original plan, which reasoned about *draw calls*
and *CPU submission*. Those don't matter — we could halve draw-call count and
gain nothing. **Only the GPU's vertex+fragment workload moves the frame rate.**
Frustum culling helped not because it cut CPU submission but because it cut GPU
geometry.

---

## Cost attribution (measured)

Each pass was gated off and the benchmark re-run:

| Config | avg FPS | mean GPU ms | Interpretation |
|---|---|---|---|
| All on (baseline) | 30 | 30 | — |
| **Vegetation off** | **100** | **9** | veg ≈ **70%** of GPU frame time (~21 ms) |
| Water off | 30 | 30 | water is negligible at the margin (~0.3 ms) |
| Terrain off | 31 | 30 | terrain is negligible at the margin (~0.5 ms) |

Then, within vegetation (trees use 11 m spacing, shrubs 4 m):

| Config | avg FPS | mean GPU ms |
|---|---|---|
| Shrubs off (trees only) | **100** | 8 |

**Shrubs are essentially the entire vegetation cost.** Trees-only ≈ no-vegetation
(~100 FPS both). The ~73K shrub instances/scene (dense 3.5–5.3K-vert meshes)
dominate everything else combined.

### Consequences

- **Terrain LOD is not worth doing.** Terrain is <2% of frame time even with no
  LOD and full 129×129 resolution on every chunk. Skip it.
- **Water optimization is not worth doing.** Same.
- **All future effort should target shrub vertex throughput.** Nothing else moves
  the needle until shrubs are cheaper.

---

## Lever sweeps (measured)

### Shrub spacing — the big lever (TRIVIAL effort)

Changing one constant in `src/world_core/content/flora.rs:57`:

| Shrub spacing | avg FPS | Δ vs baseline | ~instances |
|---|---|---|---|
| 4 m (current) | 30 | — | 100% |
| 5 m | 42 | **+38%** | ~64% |
| 6 m | 57 | **+86%** | ~44% |
| 8 m | 77 | **+154%** | ~25% |

Instance count scales with spacing⁻², so 4 m→6 m removes ~56% of shrubs and
nearly doubles FPS. The only cost is visual undergrowth density. This is the
highest value-to-effort change available, full stop.

### LOD distance threshold — weak as currently built (LOW effort)

Pulling `LOD_THRESHOLD_SQ` (`instanced_pass.rs:19`) inward:

| LOD distance | avg FPS |
|---|---|
| 512 m (current) | 30 |
| 256 m | 33 |
| 0 m (everything LOD) | 35 |

Even forcing *every* shrub to the LOD mesh only reaches 35 FPS. **The LOD mesh
isn't cheap enough** — `simplify_for_lod()` (`plant_gen/config.rs:97`) only does
`max_depth−1`, `branches≤2`, `density×0.4`, `stems≤3`, leaving the "cheap" mesh
~80% as expensive as full detail. The LOD *system* works; the LOD *target* is too
timid.

### Combined

Shrub 6 m + LOD@256 m → **65 FPS** (vs 30 baseline), i.e. the levers stack.

---

## What's actually implemented (audit, corrects the old table)

| Optimization | Old doc said | Reality |
|---|---|---|
| Frustum culling | Done | ✅ Done — terrain/instanced/water passes |
| Distance-based LOD | "—" / "Done (2-level)" (contradicted itself) | ✅ Done, 2-level @ 512 m — but cheap tier too timid (see above) |
| Foliage blob cap | "—" | ✅ Done — `tree.rs:38`, cap 500 + O(n²) enclosure cull |
| Shrub spacing | "—" (TODO) | Implemented at 4 m; **this is the lever to turn**, not a missing feature |
| Simplified world meshes | "—" | ❌ Not done — editor and world share the same full-detail config |
| Terrain LOD | "Done (2-level)" in table | ❌ Not done — **and not worth doing** (measured <2%) |
| Billboards | "—" | ✅ Done for shrubs — crossed-quad billboards (avg FPS 30.9 → 100.0); see [SHRUB_BILLBOARD.md](SHRUB_BILLBOARD.md) |

---

## Recommended priority (re-derived from data)

| # | Action | Where | Effort | Expected | Notes |
|---|---|---|---|---|---|
| **1** | **Increase shrub spacing 4 m → 6 m** | `flora.rs:57` | **Trivial** | **+86% FPS (30→57)** | Do this first. Pure visual-density tradeoff; one constant. |
| 2 | Make the LOD shrub mesh genuinely cheap (aggressive `max_depth`/foliage cut) **and** pull LOD distance to ~256 m | `config.rs:97`, `instanced_pass.rs:19` | Low | Stacks to ~65 FPS with #1 | Current LOD mesh is barely cheaper than full. |
| 3 | Distance-cull shrubs entirely past ~200–300 m (separate band for shrub species) | `instanced_pass.rs` | Low | High | Shrubs are invisible from altitude; preserves near-field look better than a uniform spacing increase. Alternative to / combine with #1. |
| 4 | Cheaper *base* shrub mesh for world rendering (decouple from editor full-detail) | `plant_gen` + registry | Medium | High | Attacks per-instance cost instead of count; keeps density. |
| ~~5~~ | ~~Billboards/impostors for distant shrubs~~ | new render path | Major | High at scale | ✅ **Done** — all shrubs now render as crossed-quad billboards (avg FPS 30.9 → 100.0, the vegetation-off ceiling). This superseded #1–#4: shrub vertex throughput is no longer the bottleneck. See [SHRUB_BILLBOARD.md](SHRUB_BILLBOARD.md). |
| ~~—~~ | ~~Terrain LOD~~ | — | — | **~0** | Measured <2% of frame. Do not pursue. |
| ~~—~~ | ~~Water optimization~~ | — | — | **~0** | Measured <1% of frame. Do not pursue. |

### Bottom line

**Resolved by #5 (shrub billboards).** Routing all shrubs to crossed-quad
billboards took avg FPS 30.9 → 100.0 (the vegetation-off ceiling) and moved the
renderer off being GPU-bound — so #1–#4 (shrub spacing / cheaper LOD / distance
cull / cheaper base mesh) are no longer needed to attack shrub cost. The next
bottleneck is elsewhere; re-run `bun tools/bench.ts` and re-attribute with the
gating method above (temporary env vars on the passes / parameters) before
chasing further wins.
