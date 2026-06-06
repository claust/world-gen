# Base-world chunk generation — Phase 2 & 3 (planned)

Phase 1 moved terrain height's two low-frequency octaves (continental + ridge)
onto a precomputed coarse global field, dropping height from 3 noise evals/vertex
to 1 (~32 s → ~22 s for the full base build). See `docs/CHUNK_GEN_PROFILING.md`
("Implemented: Phase 1") for the shipped design and `world_core::terrain_fields`.

These are the two follow-ups that were scoped but deliberately deferred. Both
remain **lossy** (the byte-identity constraint was already dropped), so each one
that lands bumps `BaseGenerationInputs.version` and reships `world_base.bin`.

## Phase 2 — moisture goes global

**Goal:** eliminate the remaining per-vertex noise on the moisture path. Moisture
is two octaves — base (~526 m wavelength) and variation (~105 m) — and *both* are
low-frequency relative to the 2 m vertex spacing, so neither needs full-res noise.
Bake both onto coarse global fields exactly like Phase 1's continental/ridge, then
per vertex moisture becomes **0 noise evals** (two bilinear lookups + the existing
weight/clamp).

**Expected:** removes moisture's ~1.8 ms/chunk (the largest single remaining cost
after Phase 1). Combined with Phase 1 this is what reaches the original
~10–13 s projection for the full build (~3–4× vs post-#100, ~5× vs the original
~47 s).

**The one risk to validate:** the variation octave (~105 m wavelength) is the
tightest of the four low-freq octaves. At the shared 32 m node spacing that's only
~3.3 nodes per wavelength, so bilinear interpolation could visibly band the biome
map (moisture drives biome classification). Mitigations, in order of preference:
give the variation field its own finer resolution; or keep variation as a per-vertex
eval and globalise only the base octave; or accept the banding if it proves
invisible in practice. Prototype in the harness first (an E9 mirroring E8) and
check both the height-style error metric *and* a biome-classification diff before
committing.

**Shape of the work:** extend `TerrainFields` (or add a sibling) with the two
moisture grids; rewrite `Heightmap::sample_moisture` / `moisture_grid` to bilinear
lookups; the `shared()` cache key gains the moisture frequencies; bump `version`.

## Phase 3 — faster detail octave (independent)

**Goal:** make the *one* remaining full-res eval cheaper. After Phases 1–2 the
detail octave (~55 m) is the only per-vertex noise left. Because the global tile
now provides the seamless world wrap, the detail octave no longer needs the 4D
torus trick — it can use a fast **2D** noise backend.

**Candidate:** `fastnoise-lite` (OpenSimplex2, pure Rust, builds on native arm64 /
x86 / wasm). The constraint that ruled out the fast 2D crates — the 4D wrap — is
gone for this octave once Phase 1/2 land.

**Trade:** a non-wrapping 2D detail octave leaves a cosmetically invisible seam in
the *finest* detail at the 65 km world boundary (the low-freq fields still wrap
exactly, so terrain shape is continuous; only the 6 m-amplitude roughness mismatches
across the seam). Almost certainly imperceptible, but worth a screenshot at the
seam before committing.

**Note:** this is orthogonal to Phases 1–2 and to the optional `simdnoise`-in-CI
lever (which speeds the *shipped* snapshot bake on x86 without changing the
algorithm). It can land before or after Phase 2.
