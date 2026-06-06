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
PROFILE_COARSE=16 cargo run --release --example profile_chunks   # tune experiment E6
```

`PROFILE_RIVER_RES` only changes the one-time river-field solve so the harness
starts fast; per-chunk river sampling is a bilinear lookup independent of that
grid, so it does not affect the chunk-gen numbers.

The harness runs six experiments:

- **E1** — full `generate_chunk` throughput and thread scaling.
- **E2** — per-layer breakdown (terrain / biome / content).
- **E3** — terrain sub-breakdown (height vs moisture vs river sampling).
- **E4** — nested-parallelism cost (inner per-vertex `par_iter` under an outer
  per-chunk `par_iter`).
- **E5** — bit-identical noise optimization: precomputed per-axis torus trig.
- **E6** — lossy optimization: coarse low-frequency octaves + full-res detail.

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

## Options

| # | Option | Speedup | Bit-identical? | Notes |
|---|---|---|---|---|
| **A** | **Precompute per-axis torus trig** (E5) | **~1.8× overall** | **Yes** (err 0.000000 m) | Hoist the cos/sin out of the per-vertex loop; 129+129 trig pairs per octave instead of 4/vertex. Biggest safe win. ~47 s → ~27 s. No `gen_key` / download change. |
| **B** | Single-pass terrain loop + drop bulk `base_plants.clone()` | ~5–10% | Yes | `terrain.rs` walks the grid twice recomputing world coords; merge into one pass. Low effort. |
| **C** | Make inner per-vertex `par_iter` conditional | neutral throughput | Yes | Serial inner for bulk gen, parallel inner for live streaming. Removes the nesting footgun, keeps streaming snappy. |
| **D** | Coarsen smooth fields (E6) | up to ~3× on a field | **No** — needs new snapshot | Low-freq octaves barely change over 256 m. **Height can't** be coarsened safely (ridge `1−\|n\|` crease caps error at ~3.7 m even at 8 m spacing). **Moisture can** (smooth, only feeds 5 biome thresholds) → removes ~40% of noise work, visually invisible. Requires regenerating `world_base.bin` + `gen_key` bump. |
| **E** | SIMD noise backend (e.g. fastnoise2) | potentially several× | **No** — different terrain | Changes the world and adds a dependency. Only worth it alongside a terrain redesign. |

### Recommendation
Do **A + B + C** together — all bit-identical / behavior-preserving, taking the
local build from **~47 s to roughly ~25 s** with **no change to the downloaded
snapshot or `gen_key`**, so downloads and existing saves stay compatible. Keep
**D (moisture only)** as a follow-up if the base world is ever reshipped for
other reasons. Skip **E** unless a terrain overhaul is already planned.

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
