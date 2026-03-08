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
    seed: u32,
)
```

Steps:

1. **Fast-forward growth**: For each `DeltaPlant`, compute `age = current_hour - born_hour`. Advance `stage` based on species thresholds (`seedling_hours`, `young_hours`).

2. **Spread seeds**: For each mature plant (base + delta), check if enough time has passed since `last_sim_hour` to trigger a spread check. If so, roll for seed spread and insert new `DeltaPlant`s.

3. **Update `last_sim_hour`** to `current_hour`.

### Tick frequency

Don't tick every frame. Tick once per game-hour (check `floor(current_hour) > floor(last_sim_hour)`). At default `day_speed` this means one tick every few real-seconds.

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

### Memory budget

Each `ChunkDelta` is tiny when empty (not stored). A delta with 10 added plants is ~500 bytes of JSON. Even 10,000 modified chunks would be ~5MB — well within budget.

---

## Integration Points (File-by-File)

| File | Change |
|------|--------|
| `world_core/chunk.rs` | Add `growth_stage: GrowthStage` to `PlantInstance` (default `Mature`) |
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

## Implementation Order

1. Add `GrowthStage` enum and `growth_stage` field to `PlantInstance` (set to `Mature` everywhere)
2. Add `DeltaPlant`, `ChunkDelta`, `DeltaStore` data structures
3. Add spread/growth config fields to `PlacementConfig` with defaults
4. Implement `assemble_plants()` — merge base + delta
5. Wire `DeltaStore` into `WorldRuntime`, apply deltas when chunks load
6. Implement `tick_chunk_lifecycle()` — growth advancement + seed spread
7. Update `build_plant_instances()` to use growth stage for scale
8. Update `InstancedPass` to route seedlings/young plants to LOD mesh
9. Add delta save/load via `Storage`
10. Test: fly around, speed up time, watch forests spread
