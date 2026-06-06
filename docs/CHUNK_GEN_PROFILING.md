# Base-world chunk generation — profiling notes

Local base-world generation (the 256×256 = 65,536-chunk build that runs when a
client can't download a snapshot) takes ~45–50 s. These are the results of
profiling **where that time goes** and which optimizations are worth doing.

## The experiment

`examples/profile_chunks.rs` is a standalone harness that drives the **same
public API the real base build uses** (`ChunkGenerator` and its three sub-layers
`TerrainLayer` / `BiomeLayer` / `ContentLayer`) over a few thousand distinct
chunks. It does not touch the GPU or the renderer — it isolates the CPU
generation cost so we can break it down and prototype changes.

```bash
cargo run --release --example profile_chunks
PROFILE_CHUNKS=8192 PROFILE_RIVER_RES=512 cargo run --release --example profile_chunks
PROFILE_COARSE=16 cargo run --release --example profile_chunks   # tune experiments E6/E7
```

`PROFILE_RIVER_RES` only changes the one-time river-field solve so the harness
starts fast; per-chunk river sampling is a bilinear lookup independent of that
grid, so it does not affect the chunk-gen numbers.

The harness runs seven experiments:

- **E1** — full `generate_chunk` throughput and thread scaling.
- **E2** — per-layer breakdown (terrain / biome / content).
- **E3** — terrain sub-breakdown (height vs moisture vs river sampling).
- **E4** — nested-parallelism cost (inner per-vertex `par_iter` under an outer
  per-chunk `par_iter`).
- **E5** — bit-identical noise optimization: precomputed per-axis torus trig.
- **E6** — lossy: coarse low-frequency octaves, interpolated *after* the crease.
- **E7** — lossy done right: coarse **raw** noise, crease + detail at full res.

Numbers below were measured on a 10-core Apple-silicon machine, `--release`.
They are directionally stable, not exact; re-run the harness to reproduce.

## Observations

### 1. Per-chunk generation is the entire cost
Extrapolating the measured rate to 65,536 chunks gives **~47 s**, matching the
known ~50 s build time. The one-time river-field solve at the native 2048 grid
is only **~0.58 s** — negligible. Everything that matters is per-chunk.

### 2. The cost is almost entirely 4D OpenSimplex noise
Per-chunk, single core (clean measurements):

| Stage | ms/chunk | share |
|---|---|---|
| **Terrain noise** — height (3 octaves) + moisture (2 octaves) | **~4.5** | **~98%** |
| River sample (bilinear lookup) | ~0.07 | ~1.5% |
| Biome classify (5 thresholds) | ~0.013 | <1% |
| Flora + houses (incl. a `base_plants.clone()`) | ~0.089 | ~2% |

Each vertex does **5 4D-OpenSimplex evaluations** (3 for height, 2 for
moisture) across 16,641 vertices/chunk. Biome and vegetation are rounding
error. Every meaningful optimization is about the noise.

E3 split: height ≈ 2.7 ms/chunk, moisture ≈ 1.8 ms/chunk, river ≈ 0.07 ms/chunk.

### 3. Nested parallelism is a wash at full core count (not a bug)
The base build does `par_iter` over chunks, and `terrain.rs` *also* does
`par_iter` over the 16,641 vertices inside each chunk. This **looks**
catastrophic in a naive scaling test (2 threads slower than 1), but that is a
measurement artifact: a "serial" outer loop still runs the inner `par_iter` on
the global pool, so it was never actually single-threaded.

Measured honestly at equal core count:

| Strategy | chunks/s @ 10 threads |
|---|---|
| Nested (current) | ~1379 |
| Chunk-level parallelism only (inner loop serial) | ~1401 |

So at full utilization the nesting neither helps nor hurts throughput — both
saturate all cores. It *is* a latent footgun (two fork-join barriers per chunk
× 65k chunks, and it interacts badly with custom pool sizes), but it is not
where the time is. Note the inner `par_iter` **does** help live single-chunk
streaming (one chunk → ~4.6 ms serial vs ~0.6 ms parallel), so it shouldn't be
removed outright, only made conditional.

## Options (first round)

| # | Option | Speedup | Bit-identical? | Status |
|---|---|---|---|---|
| **A** | **Precompute per-axis torus trig** (E5) | ~1.8× on noise | **Yes** (err 0.000000 m) | **Shipped (PR #100)** |
| **B** | Single-pass terrain loop + drop bulk `base_plants.clone()` | ~5–10% | Yes | **Shipped (loop merge); clone left as-is** |
| **C** | Make inner per-vertex `par_iter` conditional | neutral throughput | Yes | **Shipped (PR #100)** |
| **D** | Coarsen smooth fields (E6/E7) | up to ~3× on a field | No — needs reship | See "byte-identity dropped" below |
| **E** | Faster / SIMD noise backend | potentially several× | No — different terrain | See "byte-identity dropped" below |

The first round (**A + B + C**) was chosen specifically because it is
bit-identical and needs no reship. It shipped in PR #100; see "Implemented"
below. **D and E are now back on the table** — the project has since decided a
`world_base.bin` reship is acceptable, which changes everything (next section).

## Implemented: A + B + C

Landed in `heightmap.rs` (`height_grid` / `moisture_grid` trig-hoisting grid
samplers, with row-level parallelism that disables itself when already inside a
rayon worker) and `terrain.rs` (generate the fields through the grid samplers,
then carve rivers + scan min/max in a single pass). The dead `par.rs`
(`maybe_par_iter!`) macro module was removed.

**Byte-identity is guaranteed**, not just approximated: the grid samplers do the
exact same `f64` arithmetic as `torus_sample`, and the unit test
`grid_samplers_are_bit_identical_to_point_samplers` asserts equal `f32` *bits*
against the point samplers over several chunks (including off-origin/wrapped
ones). So no `gen_key` change and full compatibility with the shipped snapshot.

Measured outcome (10-core Apple silicon, `--release`):

| Metric | Before | After | Gain |
|---|---|---|---|
| Per-core | 4.59 ms/chunk | 2.63 ms/chunk | **1.74×** |
| Full-core base build (chunk-level parallel) | ~1401 chunks/s | ~2053 chunks/s | **1.47×** |
| Full 65,536-chunk world (extrapolated) | ~47 s | **~32 s** | −15 s |

The full-core gain (1.47×) is below the per-core gain (1.74×) because the build
is partly memory-bound at 10 threads; the trig hoist helps compute more than
bandwidth. Skipped from the bundle: the bulk `base_plants.clone()` (only ~1% of
a chunk, and `generate_chunk` is shared with streaming which needs the mutable
copy — not worth plumbing a flag through the shared API).

---

# Byte-identity dropped — pursuing raw speed (2026-06-06)

The project has decided that **regenerating and reshipping `world_base.bin` is
acceptable**, so generation output no longer has to be bit-identical to the
shipped snapshot. That removes the constraint behind the first round and opens
up the much larger lossy / different-noise wins.

## What byte-identity was actually protecting

- `world_base.bin` stores **baked plants** (each with `y` = terrain height at its
  position), **not** the heightmap. Terrain is **regenerated at runtime** every
  time a chunk streams in (`chunk_loader → generate_chunk → TerrainLayer`).
- So the real invariant is: *the terrain algorithm running at render time must
  match the one that baked the snapshot's plant `y` values* — otherwise plants
  float or sink.
- `gen_key` (`Herbarium::generation_key` → `BaseGenerationInputs`,
  `src/world_core/herbarium.rs`) hashes the generation **inputs** (seed,
  heightmap/biome/river config, plant placement) — it has **no algorithm
  version**.

**Consequence for any lossy change:** add a `const TERRAIN_ALGO_VERSION` to
`BaseGenerationInputs` and bump it whenever the terrain math changes, then
reship `world_base.bin`. The download URL is keyed on `gen_key`
(`…/world_base.bin?gen={gen_key:016x}`), so old snapshots auto-reject and a
new-code client never mixes an old snapshot with new terrain code. One-line
change; it is the *only* safety mechanism needed.

## The crux: the cost is **4D** OpenSimplex, and it is 4D because of the wrap

The world is toroidal — it tiles seamlessly with period `L = 65,536 m` on both
axes. To get that, `Heightmap::torus_sample` maps **each world axis onto a
circle** (`cos`, `sin`) and feeds the four circle coordinates to **4D**
OpenSimplex. That 4D-per-sample evaluation, ×5 octaves ×16,641 verts, is the
entire base-build cost.

This matters for backend choice: most fast noise crates are **2D/3D only**. You
cannot drop in a faster crate without either keeping 4D (few crates, smaller
win) **or** changing how the wrap is achieved. There are two independent levers:

- **Lever 1 — fewer evals per vertex** (algorithmic).
- **Lever 2 — cheaper evals** (faster crate / SIMD / fewer dimensions).

The best plan combines both, and conveniently the same idea relaxes the 4D
constraint.

## Lever 1: precompute global low-frequency fields  ← biggest single win

Of the 5 octaves, only the height **detail** octave (~55 m wavelength) is
high-frequency relative to the 2 m vertex spacing. The other four (continental
~10 km, ridge ~1.1 km, moisture base ~530 m / variation ~105 m) barely change
within a 256 m chunk and are wildly oversampled.

Precompute those low-frequency octaves **once** on a coarse **global** grid —
exactly the pattern `RiverField` already uses — then per vertex do cheap bilinear
lookups plus **only the one detail octave**. Per-vertex noise drops **5 evals →
1**.

Two measured prototypes of the per-chunk version of this idea (global is the
same math, precomputed once instead of per chunk):

| Experiment | approach | height ms/chunk | mean err | max err |
|---|---|---|---|---|
| E6 | interpolate low-freq **after** the `1−\|n\|` crease | ~0.9 | — | ~4.4 m (floor; doesn't improve with spacing) |
| **E7** | interpolate **raw** noise, crease + detail at full res | ~0.9 | **0.024 m** @32 m, **0.009 m** @16 m | ~4 m (rare ridge crests between nodes) |

E7 is the right way: raw noise is smooth and interpolates cleanly; the ridge
crease is reconstructed per-vertex. Mean error is centimetre-scale; the rare
metre-scale max is a ridge crest landing between coarse nodes (tighten node
spacing, or store the ridge field a little finer, to shrink it).

Going **global** instead of per-chunk additionally:
- removes the per-chunk coarse resampling (lookups only),
- **wraps seamlessly by construction** — a global tile indexed with `rem_euclid`
  tiles for free, so the low-freq octaves no longer need the 4D trick at all,
- **also speeds up live streaming**, since terrain regenerates while flying.

Estimated result: terrain noise ~2.5 ms/chunk (post-#100) → ~0.6–1.0 ms/chunk;
**full build ~32 s → ~10–13 s (~3–4× beyond #100, ~5× vs the original ~47 s)**.
Cost: a few MB of precomputed fields, built once in well under a second.

## Lever 2: faster noise backend

Independently, each remaining eval can be made cheaper. Candidates (Rust):

| Crate | Speed | Dims | f32/f64 | wasm? | ARM (Apple) SIMD? | Notes |
|---|---|---|---|---|---|---|
| `noise` 0.9 (current) | baseline | 1–4D | f64 | yes | n/a (scalar) | What we use; scalar, f64. |
| `fastnoise-lite` | fast scalar | **2D/3D only** | f32 | **yes** (pure Rust) | n/a (scalar) | OpenSimplex2 ≈ same look. No 4D → can't wrap in 4D; pairs perfectly with global fields (fill the tile + 2D detail). |
| `simdnoise` | **very fast, batched** | 1–4D | f32 | x86 SIMD only | **no** (x86 only) | Generates whole grids at once. Would massively speed the **CI** x86 base-build, but on the dev Mac (arm64) and wasm it falls back to scalar / won't build. |
| `fastnoise2` (FFI) | **fastest** (C++ SIMD) | up to 4D | f32 | **no** (C++ FFI) | yes (NEON) | Disqualified for the shared crate by wasm; possible CI-only path, but messy. |
| `libnoise` | modest | up to 4D | f64 | yes | n/a | Cleaner API, marginal speed gain over `noise`. |
| portable SIMD (`wide` / `std::simd`) | fast, cross-platform | DIY | f32 | yes (SIMD128) | yes (NEON) | Hand-write value/gradient noise; OpenSimplex's gather-heavy permutation lookups SIMD-ize poorly, so this favours a different basis. |

Key constraints that fall out of the table:
- The crate must build on **native arm64 (local), x86 (CI), and wasm32**. That
  rules `simdnoise` (x86-only SIMD) and `fastnoise2` (C++/wasm) out of the
  *shared* path, though `simdnoise` could accelerate the CI snapshot bake.
- The current 4D wrap rules out the 2D/3D-only fast crates **unless** the wrap
  strategy changes — which Lever 1 does anyway.

## Recommended direction

**Global low-frequency fields + a fast 2D detail octave.** Concretely:

1. Add `const TERRAIN_ALGO_VERSION` to `BaseGenerationInputs`; bump on each
   terrain-math change.
2. Precompute global coarse fields (continental, ridge-raw, moisture base &
   variation) like `RiverField`; sample them with bilinear + `rem_euclid` wrap.
   This alone gives the 5→1 eval drop and removes the 4D requirement for those
   octaves.
3. For the per-vertex detail octave, either keep `noise` (now 1/5 of the work)
   or switch it to `fastnoise-lite` OpenSimplex2 **2D** — the global tile already
   provides the seamless wrap, so the detail octave can use a non-wrapping fast
   2D crate (a cosmetically invisible seam in the finest detail at the 65 km
   boundary is the only trade).

Expected: **~47 s → ~10–13 s** for the local build, plus faster live streaming.
Reship `world_base.bin` once (CI already regenerates it). Validate by
prototyping the global-field version in the harness first (the E7 numbers are
the per-chunk lower bound; global should match or beat them with no per-chunk
resampling).

`simdnoise` in CI is a separate, optional lever to make the *shipped* snapshot
bake cheap, independent of the algorithm above.

## Implemented: Phase 1 — global height field (continental + ridge)

Landed in `world_core::terrain_fields` (`TerrainFields`): the **raw** continental
and ridge noise are baked once onto a coarse global grid at the river-field
resolution (`HeightmapConfig::low_freq_field_resolution`, native 2048 ≈ 32 m/node
/ web 1024), then bilinearly sampled with `rem_euclid` wrap — exactly the
`RiverField` pattern, so the tile wraps seamlessly and these two octaves no longer
use the 4D torus trick. The ridge crease (`1 − |n|`), amplitudes, and the detail
octave are still reconstructed per vertex (detail is now the **only** full-res 4D
eval). `Heightmap` holds an `Arc<TerrainFields>` fetched through a memoized
`shared()` cache keyed on `(seed, continental/ridge freq, resolution)`, so the
~tens-of-MB grid is built once per world and shared by every `Heightmap`
(terrain, rivers, plant placement, spawn scans) without threading it through
their constructors.

Per the project's no-byte-identity decision this is **lossy** (mean ≈ 0.024 m,
rare ridge-crest max ≈ 4 m vs the exact field), so `BaseGenerationInputs.version`
was bumped **1 → 2**; the old local `world_base.bin` auto-rejects and regenerates.
The grid-vs-point bit-identity test still holds — both samplers now share the
field path plus an identical detail-trig hoist, so they stay in lockstep (the
real invariant: streamed terrain must match baked plant `y`).

Measured (10-core Apple silicon, `--release`, harness E1/E3):

| Metric | Before (post-#100) | After (Phase 1) | Gain |
|---|---|---|---|
| Height field (point sample, E3) | 2.7 ms/chunk | **1.0 ms/chunk** | 3 evals → 1 |
| Full 65,536-chunk build (parallel-outer) | ~32 s | **~22 s** | ~1.5× |

Moisture is untouched (still 2 evals/vertex, ~1.8 ms/chunk) — moving it global
(**Phase 2**, → 0 evals) is what reaches the doc's ~10–13 s projection; the fast
2D detail octave (**Phase 3**) is independent. The E8 experiment in
`examples/profile_chunks.rs` is the global-field prototype this shipped from.
