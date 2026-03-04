# Performance Improvement Plan

## The Problem

After switching from simple geometric primitives to full procedural `plant_gen` meshes for instanced world rendering, performance dropped significantly. The detailed branching and foliage meshes are too complex for the thousands of instances rendered each frame.

### Key Numbers (load_radius=3, 49 chunks)

- Trees (Oak, Birch, Willow): ~2,000-4,200 verts, ~9K-22K indices each
- Shrubs: ~3,500-5,300 verts, ~19K-29K indices each (7 stems, each with full branch tree + dense foliage)
- Shrub instances: ~1,500/chunk at 4m spacing = ~73K total
- Tree instances: ~200/chunk in forest = ~10K total
- Per-frame: shrubs alone = ~365M vertex shader invocations, trees ~30M
- Draw calls: up to 343 plant + ~49 house = ~400 total

### Complexity Drivers (ordered by impact)

1. `max_depth` — each level multiplies branches exponentially
2. `branches_per_node` — direct multiplier on branch count
3. `crown.density` + `foliage.cluster_strategy` — `dense_mass` = 4x density blobs per endpoint
4. `stem_count` — shrubs have 3-7 stems, each a full branch tree

---

## Optimizations

### 1. Frustum Culling — DONE

Skip draw calls for chunks outside the camera's view frustum. Extracts 6 clip planes from the view-projection matrix and tests each chunk's AABB before issuing draw calls. Applied to terrain, instanced, and water passes.

**Impact:** ~50% reduction in draw calls (chunks behind camera are skipped).

### 2. Simplified World Meshes (HIGH impact, LOW effort)

Generate meshes with reduced `SpeciesConfig` parameters for instanced world rendering. Clone each species config and reduce:
- `max_depth` (reduces exponential branching)
- `branches_per_node`
- `crown.density` (sparser foliage)

The plant editor keeps full-detail meshes; the world uses lightweight versions. Could cut vertex counts 3-5x.

### 3. Reduce Shrub Density (MEDIUM impact, TRIVIAL effort)

Shrubs are placed on a 4m spacing grid (4,096 candidates/chunk). Increasing to 6-8m spacing cuts shrub instances 2-4x. Since shrubs are the biggest GPU cost driver, this alone would provide significant relief.

### 4. Foliage Blob Cap (MEDIUM impact, LOW effort)

Each foliage cluster uses an icosahedron (12 verts, 20 tris). Dense species generate hundreds of blobs per mesh. Cap total foliage blobs per mesh (e.g., 50) or merge nearby blobs to reduce vertex counts for the densest species.

### 5. Distance-Based LOD (HIGH impact, HIGH effort)

Generate 2-3 detail levels per species. Near chunks get the detailed mesh; far chunks get a simplified version. Requires per-chunk distance checks, multiple prototype meshes per species, and LOD selection logic during rendering.

### 6. Billboards for Distant Plants (HIGH impact, MAJOR effort)

Replace far-away instances with camera-facing textured quads. Requires texture baking (rendering each species to an atlas), a separate billboard render path, and camera-facing orientation updates. Industry-standard solution for large-scale vegetation.

---

## Recommended Priority

| # | Optimization | Impact | Effort | Status |
|---|---|---|---|---|
| 1 | Frustum culling | ~50% draw calls | Medium | Done |
| 2 | Simplified world meshes | 3-5x vertex reduction | Low | — |
| 3 | Reduce shrub density | 2-4x fewer instances | Trivial | — |
| 4 | Foliage blob cap | Medium vert reduction | Low | — |
| 5 | Distance-based LOD | High for far chunks | High | — |
| 6 | Billboards | Best for scale | Major | — |
