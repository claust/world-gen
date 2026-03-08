# Plant Lifecycle Design — Delta Layer Approach

## Goal

Add a lifecycle to plants: seeds spread from mature plants, land nearby, germinate, and grow through stages. Keep the first iteration minimal — build on the existing deterministic chunk generation with a lightweight overlay that tracks changes.

## Core Idea

```
visible_plants(chunk) = deterministic_base(coord, seed) + delta(coord)
```

The existing `FloraLayer` continues to generate the same deterministic plants it always has. A new **delta store** records modifications on top: removed plants, new seedlings, growth stage overrides. When a chunk loads, the delta is applied. When a chunk unloads, the delta persists in memory (and optionally on disk).

---

## Growth Stages

Three stages, deliberately simple:

| Stage       | Visual                              | Duration (game-hours) | Can Spread Seeds? |
|-------------|-------------------------------------|-----------------------|-------------------|
| **Seedling** | 15% scale, no crown (trunk-only LOD mesh) | 48h                   | No                |
| **Young**    | 50% scale, LOD mesh with crown      | 96h                   | No                |
| **Mature**   | 100% scale, full mesh (current)     | Indefinite            | Yes               |

Deterministic base plants start as **Mature** — they represent an established forest. Only delta-tracked plants (spread from seeds or player-planted) go through the growth stages.

The growth durations are per-species and configured in `PlacementConfig`, with the values above as defaults.

---

## Data Structures

### PlantLifecycle (new, in `world_core`)

```rust
/// Growth stage of a delta-tracked plant.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum GrowthStage {
    Seedling,
    Young,
    Mature,
}

/// A plant that exists in the delta layer (not in the deterministic base).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeltaPlant {
    pub position: Vec3,
    pub rotation: f32,
    pub height: f32,           // target mature height
    pub species_index: usize,
    pub stage: GrowthStage,
    pub born_hour: f64,        // world-hour when seed landed
}
```

### ChunkDelta (new, in `world_core`)

```rust
/// Overlay of modifications to a chunk's deterministic plant content.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChunkDelta {
    /// Indices into the deterministic base plant list that have been removed
    /// (e.g. chopped down by player). Sorted, deduplicated.
    pub removed_base: Vec<usize>,

    /// Plants added by the lifecycle system or the player.
    pub added_plants: Vec<DeltaPlant>,

    /// World-hour when this chunk was last simulated.
    pub last_sim_hour: f64,
}
```

### DeltaStore (new, in `world_runtime`)

```rust
/// Global store of all chunk deltas. Lives for the entire session.
/// Chunks that have never been modified have no entry (zero cost).
pub struct DeltaStore {
    deltas: HashMap<IVec2, ChunkDelta>,
}

impl DeltaStore {
    pub fn get(&self, coord: &IVec2) -> Option<&ChunkDelta>;
    pub fn get_or_create(&mut self, coord: IVec2) -> &mut ChunkDelta;
    pub fn is_empty(&self) -> bool;

    /// Persist to Storage (JSON). Called on game save.
    pub fn save(&self, storage: &dyn Storage) -> anyhow::Result<()>;
    pub fn load(storage: &dyn Storage) -> Self;
}
```

---

## Seed Spread Rules

When the lifecycle simulation ticks a chunk, each **Mature** base plant and each **Mature** delta plant can produce seeds.

### First-iteration rules (simple)

```
spread_check_interval:  24 game-hours per mature plant
spread_chance:          0.3 (30% chance per check)
spread_count:           1-2 seeds per successful check
spread_radius:          10-40m (species-dependent, configurable in PlacementConfig)
```

### RNG Determinism Contract

Seed spread uses the same `hash4` + `hash_to_unit_float` pattern as `FloraLayer` — no `rand` crate. Every random decision is a pure function of fixed inputs, so the same world state always produces the same spread results. This matters for save/load stability: a save captures `DeltaStore` + `total_hours`, and replaying ticks from that state must yield identical deltas.

**Seed offset range:** Lifecycle uses offsets **4000–4999** (flora uses 0–2999, houses use 1001–3999). All lifecycle hashes follow:

```
hash_to_unit_float(hash4(world_seed.wrapping_add(OFFSET), input_a, input_b, input_c))
```

**Per-decision inputs:**

| Decision | Offset | a | b | c | Notes |
|----------|--------|---|---|---|-------|
| Spread chance | 4001 | chunk_x | chunk_z | plant_index | Roll < `spread_chance` to produce seeds |
| Seed count | 4002 | chunk_x | chunk_z | plant_index | Maps to 1–2 via `1 + floor(v * 2)` |
| Landing angle | 4101 | chunk_x | chunk_z | plant_index ⊕ seed_i | `v * TAU` for each seed_i (0, 1) |
| Landing distance | 4102 | chunk_x | chunk_z | plant_index ⊕ seed_i | `sqrt(v) * spread_radius` (uniform-area disk) |
| Species height | 4201 | chunk_x | chunk_z | plant_index ⊕ seed_i | Lerp within species `height_range` |
| Rotation | 4202 | chunk_x | chunk_z | plant_index ⊕ seed_i | `v * TAU` |

Where:
- `chunk_x`, `chunk_z` are the **source** chunk coordinates (as `u32`).
- `plant_index` is the plant's index in the combined (base + delta) list for that chunk, cast to `u32`.
- `seed_i` is the seed ordinal (0 or 1) within one spread event. Combined via `plant_index.wrapping_mul(31).wrapping_add(seed_i)` — same sub-id pattern as hamlet house placement.
- `⊕` denotes the wrapping_mul+wrapping_add sub-id combinator above.

**Hour bucketing:** Spread is evaluated once per game-hour boundary (`floor(current_hour) > floor(last_sim_hour)`). The hour value is **not** an RNG input — it controls *when* the tick fires, not the hash output. This means a tick at hour 100.3 and one at hour 100.9 produce the same spread results for the same chunk state. The hour bucket implicitly serializes spread rounds: hour 100's seeds exist before hour 101's spread runs, so growth is ordered.

**Catch-up ticks:** When `current_hour - last_sim_hour > 1`, the simulation loops over each missed hour boundary sequentially. Each round uses the same hash inputs (chunk, plant_index) but operates on updated state from the previous round (new delta plants from round N are mature-eligible in round N + seedling_hours + young_hours). This is deterministic because the loop order is fixed: hours ascending, chunks in HashMap iteration order (order doesn't matter — chunks don't interact within a single hour boundary, only via cross-chunk seed landing which is applied lazily).

**What is NOT seeded by hash4:** Growth stage advancement is purely time-based (`age >= threshold`), not random. No RNG involved.

### Seed landing logic

1. Pick a random angle and distance within `spread_radius`.
2. Compute landing position (can cross chunk boundaries).
3. Sample terrain at landing position — check height, moisture, slope, biome.
4. Run the same eligibility filter as `FloraLayer` (biome match, moisture range, altitude range, slope).
5. Check minimum spacing from existing plants (both base and delta) — e.g. 3m for shrubs, 8m for trees.
6. If all checks pass, insert a `DeltaPlant` with `stage: Seedling` into the target chunk's delta.

Seeds that land in **unloaded chunks** still write to the `DeltaStore` — the delta for that chunk is created lazily. The terrain eligibility check is skipped for unloaded chunks (deferred to when the chunk loads, at which point invalid seedlings are pruned).

### PlacementConfig additions

```rust
// Added to existing PlacementConfig:
pub spread_radius: f32,       // default: 25.0m
pub spread_chance: f32,       // default: 0.3
pub seedling_hours: f32,      // default: 48.0
pub young_hours: f32,         // default: 96.0
```

---

## Simulation Tick

### Where it runs

In `WorldRuntime::update()`, after streaming update. Only ticks **loaded chunks**.

### Per-chunk tick logic

```
fn tick_chunk_lifecycle(
    coord: IVec2,
    base_plants: &[PlantInstance],
    delta: &mut ChunkDelta,
    current_hour: f64,
    registry: &PlantRegistry,
    terrain: &ChunkTerrain,
    biome_map: &BiomeMap,
    delta_store: &mut DeltaStore,   // for cross-chunk seed spread
    seed: u32,                      // world_seed — used as hash4 base, see RNG contract
)
```

Steps:

1. **Catch-up loop**: Compute `missed = floor(current_hour) - floor(last_sim_hour)`. Loop over each missed hour boundary sequentially (not batched) to preserve deterministic ordering — see RNG contract above.

2. **Fast-forward growth** (per round): For each `DeltaPlant`, compute `age = round_hour - born_hour`. Advance `stage` based on species thresholds (`seedling_hours`, `young_hours`). This is pure arithmetic, no RNG.

3. **Spread seeds** (per round): For each mature plant (base + delta), roll `hash4(seed + 4001, chunk_x, chunk_z, plant_index)` for spread chance. On success, generate 1–2 seeds with landing positions derived from offsets 4101–4202. Insert new `DeltaPlant`s into the appropriate chunk's delta.

4. **Update `last_sim_hour`** to `current_hour`.

### Tick frequency

Don't tick every frame. Tick once per game-hour (check `floor(current_hour) > floor(last_sim_hour)`). At default `day_speed` this means one tick every few real-seconds.

### Lifecycle Invariants

These invariants must hold at all times. Violations are bugs — assert them in debug builds.

1. **No negative ages.** `born_hour <= current_total_hours` for every `DeltaPlant`. A plant cannot be born in the future. Enforced at insertion: `born_hour` is set to the current `total_hours` at the moment the seed lands.

2. **Stage monotonicity.** Growth stages only advance: `Seedling → Young → Mature`. A plant's stage never regresses. There is no code path that sets a lower stage. (Death/decay would add a terminal stage in a future iteration — it would not reuse existing stages.)

3. **No duplicate seedlings within min spacing.** Before inserting a `DeltaPlant`, check minimum distance to all existing plants (base + delta) in the target chunk. Use species-appropriate spacing (e.g. 8m for trees, 3m for shrubs — same values as `FloraLayer`'s grid spacing). If a position is too close to an existing plant, the seed is silently discarded. This prevents clumping from multiple spread rounds targeting the same area.

4. **Stable prune-on-load.** When a chunk loads and its delta contains plants that landed in unloaded terrain (eligibility was deferred), run the same `FloraLayer` eligibility filter (biome, moisture, altitude, slope) and min-spacing check against the now-available terrain data. Pruning must be **idempotent**: loading the same chunk twice with the same delta and terrain produces the same result. Pruned plants are removed from `added_plants` permanently (the delta is mutated, not filtered transiently). This means a save after prune will not contain the invalid plants.

5. **`removed_base` indices are valid.** Every index in `removed_base` must be `< base_plants.len()` for that chunk. If base generation changes (e.g. species registry update), stale indices are pruned on load. Assert `removed_base` is sorted and deduplicated after any mutation.

6. **`last_sim_hour` never exceeds `total_hours`.** The simulation cannot run ahead of the world clock. After each tick: `delta.last_sim_hour <= runtime.total_hours`.

7. **Finite catch-up bound.** Cap the catch-up loop at a configurable maximum (default: 500 hours). If `current_hour - last_sim_hour > max_catchup_hours`, clamp to `max_catchup_hours` and log a warning. This prevents a long-idle save from freezing the game on load with thousands of tick rounds.

---

## Rendering Integration

### Scale factor from growth stage

The existing instancing pipeline computes `scale = plant_height / reference_height`. For delta plants, multiply by a stage factor:

| Stage    | Scale Multiplier |
|----------|-----------------|
| Seedling | 0.15            |
| Young    | 0.50            |
| Mature   | 1.00            |

### Mesh selection by stage

- **Seedling**: Use the LOD mesh (already exists, simpler geometry). At 15% scale this reads as a small sprout.
- **Young**: Use the LOD mesh at 50% scale.
- **Mature**: Use the full-detail mesh.

No new meshes needed for the first iteration.

### PlantInstance changes

Add one field to `PlantInstance`:

```rust
pub struct PlantInstance {
    pub position: Vec3,
    pub rotation: f32,
    pub height: f32,
    pub species_index: usize,
    pub growth_stage: GrowthStage,  // NEW — defaults to Mature for base plants
}
```

The instanced pass already partitions plants by LOD distance. Extend this to also partition by growth stage within each chunk, so seedlings/young plants use the LOD mesh regardless of distance.

### Chunk content assembly

When a chunk loads or its delta changes, rebuild the effective plant list:

```rust
fn assemble_plants(
    base: &[PlantInstance],
    delta: &ChunkDelta,
) -> Vec<PlantInstance> {
    let mut plants: Vec<PlantInstance> = base
        .iter()
        .enumerate()
        .filter(|(i, _)| !delta.removed_base.contains(i))
        .map(|(_, p)| p.clone())
        .collect();

    for dp in &delta.added_plants {
        plants.push(PlantInstance {
            position: dp.position,
            rotation: dp.rotation,
            height: dp.height * dp.stage.scale_factor(),
            species_index: dp.species_index,
            growth_stage: dp.stage,
        });
    }

    plants
}
```

This replaces the raw `chunk.content.plants` when building GPU instance buffers in `sync_chunks`.

### Chunk ownership clarification

To support deterministic base indexing and cheap re-assembly, loaded chunks should keep **both**:

- `base_plants`: the deterministic flora generated from world seed + chunk coord
- `plants`: the assembled visible list after applying `ChunkDelta`

That means the effective runtime model is:

```rust
pub struct ChunkContent {
    pub base_plants: Vec<PlantInstance>,
    pub plants: Vec<PlantInstance>,
    pub houses: Vec<HouseInstance>,
}
```

`removed_base` indices always refer to `base_plants`, never the assembled `plants` list.

### Renderer invalidation clarification

The current renderer caches per-chunk instance buffers and only rebuilds them when a chunk first appears. Lifecycle updates need an explicit invalidation signal, otherwise delta changes will not become visible.

Recommended approach: add a `content_revision: u64` (or equivalent dirty version) to each loaded chunk. Whenever assembled visible plants change, increment the revision. The instanced renderer tracks the last uploaded revision per chunk and rebuilds instance buffers when the revision changes.

---

## Persistence

### Save format

Add to `SaveData`:

```rust
pub struct WorldSave {
    pub seed: u32,
    pub hour: f32,
    pub day_speed: f32,
    pub total_hours: f64,          // NEW — cumulative world hours since creation
}
```

`DeltaStore` saves separately via `Storage` under key `"deltas"`. Format: JSON map of `"x,y" -> ChunkDelta`. Only non-empty deltas are stored.

### Simulation clock semantics

`total_hours` is the **authoritative clock** for all lifecycle operations. It is the single source of truth for:

- **Aging:** `DeltaPlant.born_hour` is compared against `total_hours` to compute age and determine growth stage. `born_hour` values are always written as the `total_hours` at insertion time.
- **Tick scheduling:** `ChunkDelta.last_sim_hour` and the current `total_hours` determine whether a tick fires and how many catch-up rounds to run.
- **Spread windows:** The spread check interval (24 game-hours) is measured in `total_hours` units.

`total_hours` is **monotonically increasing** — it never rewinds. It advances by `delta_time * day_speed` each frame, accumulated as `f64` to avoid precision loss over long sessions. On save, `total_hours` is persisted. On load, it is restored exactly — the lifecycle system resumes from where it left off.

**Relationship to `hour`:** The existing `hour: f32` field is the time-of-day cycle (0.0–24.0, wrapping). It drives lighting and sky rendering. The lifecycle system does **not** use `hour` — it uses `total_hours` exclusively. The two are related by: `hour ≈ total_hours % 24.0` (modulo float precision), but `total_hours` is the canonical reference.

**New-world initialization:** When a fresh world is created, `total_hours` starts at `hour / 1.0` (matching the initial time-of-day). All `ChunkDelta.last_sim_hour` values default to `0.0`, so the first tick for each chunk will catch up from hour 0 to the current `total_hours` — which for a new world is near-zero, producing no spread (no time has elapsed).

**Save migration:** Existing saves lack `total_hours`. On load, if missing, compute `total_hours = hour` (assume the world has existed for less than one day cycle). This is an approximation but safe: it means existing deltas (there are none in pre-lifecycle saves) would at most lose a few hours of catch-up, and base plants are unaffected.

### Memory budget

Each `ChunkDelta` is tiny when empty (not stored). A delta with 10 added plants is ~500 bytes of JSON. Even 10,000 modified chunks would be ~5MB — well within budget.

---

## Integration Points (File-by-File)

| File | Change |
|------|--------|
| `world_core/chunk.rs` | Add `growth_stage: GrowthStage` to `PlantInstance` (default `Mature`) |
| `world_core/chunk.rs` | Split `ChunkContent.plants` into `base_plants` + assembled visible `plants`; add `content_revision` |
| `world_core/herbarium.rs` | Add spread/growth fields to `PlacementConfig` with defaults |
| `world_core/lifecycle.rs` | **NEW** — `GrowthStage`, `DeltaPlant`, `ChunkDelta`, `assemble_plants()`, `tick_chunk_lifecycle()` |
| `world_runtime/delta_store.rs` | **NEW** — `DeltaStore` with save/load |
| `world_runtime/runtime.rs` | Own a `DeltaStore`, call lifecycle tick in `update()`, expose assembled chunk content |
| `world_runtime/streaming.rs` | After polling new chunks, apply deltas before handing to renderer |
| `renderer_wgpu/instanced_pass.rs` | Use `growth_stage` to select mesh variant (full vs LOD) and apply scale multiplier |
| `renderer_wgpu/instancing.rs` | `build_plant_instances()` reads `growth_stage` for scale factor |
| `world_core/save.rs` | Add `total_hours` to `WorldSave` |
| `world_core/content/flora.rs` | Set `growth_stage: Mature` on all generated base plants |

---

## What This Does NOT Include (Future Iterations)

- **Player interaction** (planting seeds, chopping trees) — needs input handling, not lifecycle logic
- **Death/decay stage** — plants currently live forever once mature
- **Competition** (plants shading out neighbors) — would need density checks
- **Seasonal variation** (leaf color changes, dormancy)
- **Cross-chunk terrain sampling for seed spread** — first iteration skips eligibility for unloaded target chunks
- **Visual seed/fruit on mature plants** — mesh changes
- **Different meshes per growth stage** — first iteration reuses LOD meshes at smaller scale

---

## Implementation Phases

Each phase should be independently shippable and leave the codebase in a working state.

### Phase 1 — Data Model + Save Foundations

Goal: introduce lifecycle types and clock semantics without changing gameplay behavior.

Deliverables:

1. Add `GrowthStage` and `growth_stage` to `PlantInstance`; all deterministic flora defaults to `Mature`
2. Add `DeltaPlant` and `ChunkDelta`
3. Add lifecycle fields to `PlacementConfig` with defaults:
   - `spread_radius`
   - `spread_chance`
   - `seedling_hours`
   - `young_hours`
4. Add `total_hours` to `WorldSave`
5. Extend `WorldClock` to track both wrapped `hour` and monotonic `total_hours`
6. Add save migration: if `total_hours` is missing, initialize it from `hour`

Acceptance criteria:

- Existing worlds load normally
- New saves write `total_hours`
- Rendering is unchanged
- No lifecycle simulation runs yet

### Phase 2 — Delta Store + Chunk Assembly

Goal: make runtime chunks lifecycle-ready by separating deterministic base flora from assembled visible flora.

Deliverables:

1. Split loaded chunk plant content into:
   - `base_plants` for deterministic flora
   - `plants` for assembled visible flora
2. Implement `DeltaStore` with JSON persistence under storage key `"deltas"`
3. Implement `assemble_plants(base, delta)`
4. When a chunk loads, apply its delta and rebuild visible plants
5. Prune stale `removed_base` indices on load if they exceed `base_plants.len()`

Acceptance criteria:

- With no deltas, visible output matches pre-lifecycle behavior
- Chunks can be deterministically re-assembled from base + delta
- Delta persistence round-trips cleanly

### Phase 3 — Renderer Integration + Invalidation

Goal: make lifecycle-visible changes render correctly and predictably.

Deliverables:

1. Add chunk-level revision/dirty tracking when assembled visible plants change
2. Update instancing to rebuild GPU buffers when a chunk revision changes
3. Apply growth-stage scale factors during instance generation
4. Route `Seedling` and `Young` plants to LOD meshes regardless of distance
5. Keep `Mature` plants on the existing full-vs-LOD distance path

Acceptance criteria:

- Editing a chunk's visible plant list updates the scene without reloading the whole world
- Seedlings and young plants render with the intended smaller LOD presentation
- No stale GPU instance buffers remain after delta changes

### Phase 4 — Growth Advancement

Goal: support time-based stage progression for delta plants before adding reproduction.

Deliverables:

1. Implement stage advancement from `born_hour` + species thresholds
2. Add per-chunk lifecycle catch-up based on `last_sim_hour`
3. Enforce lifecycle invariants in debug builds:
   - no negative ages
   - stage monotonicity
   - `last_sim_hour <= total_hours`
   - bounded catch-up
4. Re-assemble chunks after stage changes so rendering reflects growth

Acceptance criteria:

- Delta plants move from `Seedling -> Young -> Mature` as `total_hours` advances
- Loading an older save catches up growth deterministically within the catch-up cap
- No spreading occurs yet

### Phase 5 — Seed Spread In Loaded Chunks

Goal: enable deterministic reproduction within the loaded simulation area.

Deliverables:

1. Implement `tick_chunk_lifecycle()`
2. Run lifecycle ticks from `WorldRuntime::update()` on loaded chunks only
3. Apply deterministic spread chance, seed count, landing angle/distance, height, and rotation using the documented `hash4` offsets
4. Check eligibility against terrain, biome, moisture, altitude, slope, and min spacing
5. Insert successful landings as `DeltaPlant { stage: Seedling, born_hour: total_hours }`

Acceptance criteria:

- Mature plants in loaded chunks can reproduce deterministically
- Replaying from the same save state yields the same delta results
- Growth + spread ordering remains stable during catch-up

### Phase 6 — Cross-Chunk Landing + Prune-On-Load

Goal: finish the lazy cross-chunk behavior for unloaded targets.

Deliverables:

1. Allow seeds from loaded chunks to land in unloaded target chunks by creating lazy delta entries
2. Defer terrain validation for unloaded targets
3. On chunk load, prune invalid deferred seedlings using the same placement eligibility checks
4. Ensure prune-on-load is idempotent and permanent

Acceptance criteria:

- Unloaded target chunks can accumulate deferred seedlings
- Loading the same chunk twice produces the same pruned result
- Invalid deferred seedlings are removed from persisted deltas

### Phase 7 — Debugging, Validation, and Tuning

Goal: make the system observable and safe to iterate on.

Deliverables:

1. Add unit tests for:
   - save migration
   - delta persistence
   - assembly
   - growth progression
   - deterministic spread
2. Add runtime logging for catch-up clamps and invalid delta pruning
3. Verify behavior using the existing screenshot/debug CLI workflow:
   - accelerate `day_speed`
   - observe stage transitions
   - confirm visible spread over time
4. Tune default spread and stage timings if needed

Acceptance criteria:

- The lifecycle system has deterministic test coverage at the core logic layer
- Long-idle saves do not hang the game
- Visual verification loop is practical for iteration
