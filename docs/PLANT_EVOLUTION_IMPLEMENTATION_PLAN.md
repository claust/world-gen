# Plant Evolution - First Version Implementation Plan

> Status: implementation planning. This document turns the first-version scope in
> `PLANT_EVOLUTION.md` into a technical plan for the current Rust/wgpu runtime.
> It compares implementation options, then recommends a staged build.

## First Version Scope

The first playable version should ship a visible, debuggable evolutionary loop:

- Eight named ecological genes.
- A genotype -> phenotype function with explicit trade-offs.
- Fitness from moisture, altitude, and local competition.
- Inheritance by parent copy plus mutation.
- Selection through seed count, seed landing, establishment, growth, stress
  color, lifespan, and spread chance.
- Stored genes for every plant, including the generated base flora.
- Region readout and at least one evolution overlay.

The goal is not to make every possible biology feature in one pass. The goal is
to make a closed loop where plants inherit variation, that variation changes
their realized life history, the environment and neighbours select among them,
and the player can see populations move through genotype/phenotype space.

## Current Technical Context

The existing architecture is well suited to this feature:

- `src/world_core/` owns pure world data and generation rules.
- `src/world_runtime/plant_world.rs` owns the resident whole-world plant store,
  lifecycle, spread, save/load, and per-chunk reconstruction for rendering.
- Base plants are the prefix of each chunk's plant list. Spreading plants are the
  suffix and are the only plants persisted in `plants.bin`.
- Spread is already a two-phase process:
  - `emit_chunk_candidates()` reads mature parents and emits deterministic seed
    candidates.
  - `land_chunk_candidates()` validates landing sites and appends accepted
    seedlings.
- Rendering already supports per-instance scale and color through
  `PlantInstance` -> `InstanceData`.
- The debug API already publishes lifecycle telemetry and supports command
  responses with arbitrary JSON payloads.

That means evolution can be added inside the existing plant lifecycle rather
than as a separate simulation engine.

## Data Model

Add a small evolution module in `world_core`:

```text
src/world_core/evolution.rs
```

Core types:

```rust
pub struct PlantGenes {
    pub wet_pref: u8,
    pub alt_pref: u8,
    pub stress_width: u8,
    pub capture: u8,
    pub fecundity: u8,
    pub seed_mass: u8,
    pub dispersal: u8,
    pub timing: u8,
}

pub struct PlantPhenotype {
    pub abiotic_fitness: f32,
    pub competition_room: f32,
    pub stress: f32,
    pub height_scale: f32,
    pub width_scale: f32,
    pub maturity_scale: f32,
    pub lifespan_scale: f32,
    pub seed_count_scale: f32,
    pub spread_radius_scale: f32,
    pub establishment_chance: f32,
    pub leaf_tint: [f32; 4],
}

pub struct PlantEnvironment {
    pub moisture: f32,
    pub altitude: f32,
    pub river_wetness: f32,
    pub shade: f32,
    pub root_pressure: f32,
}
```

Genes should be stored as bytes. Converting `u8 -> f32` at use sites is cheap,
keeps persistence compact, and gives enough resolution for gradual evolution.

### Exact Gene Byte Layout

Store genes directly on `Plant` as eight bytes in this fixed order:

```text
0 wet_pref
1 alt_pref
2 stress_width
3 capture
4 fecundity
5 seed_mass
6 dispersal
7 timing
```

`PlantGenes` can be a small named-field wrapper for readability, but persistence
should treat it as this stable byte sequence. Use helper methods for conversion:

```rust
impl PlantGenes {
    pub const BYTE_LEN: usize = 8;
    pub fn from_bytes(bytes: [u8; 8]) -> Self;
    pub fn to_bytes(self) -> [u8; 8];
    pub fn unit(self_field: u8) -> f32; // byte -> 0..1
}
```

Avoid `repr(C)` assumptions for serialization. Write/read the eight bytes
explicitly so future field additions require an intentional format bump.

## Storage Research: Three Ways To Store Genes

### Option A: Inline Genes In `Plant`

Extend `Plant` with `genes: PlantGenes`.

Pros:

- Straightforward code: every plant carries everything needed.
- Parent inheritance, rendering, inspection, save/load are direct.
- No lookup table or parallel vector can get out of sync.

Cons:

- `Plant` grows from 16 bytes to roughly 24 bytes.
- Base world memory grows too if every base plant stores genes.
- Base snapshot format needs a version bump and regeneration.

This is the simplest model. Base flora is not a special ecological class; it is
just the initial population. Every plant can be inspected, rendered, selected,
and used as a parent through exactly the same code path.

### Option B: Store Genes Only For Spread Plants In A Sidecar

Keep `Plant` packed as-is. Add a parallel per-chunk vector for spread genes:

```rust
spread_genes[idx][plant_index - base_count[idx]]
```

Base plant genes are computed from deterministic hashes when needed.

Pros:

- Keeps the large base population compact.
- Persists only the evolving population's real inherited genes.
- Avoids bloating `world_base.bin`.

Cons:

- Any insertion/removal in a chunk must update two vectors consistently.
- Code must carefully distinguish base index vs spread suffix index.
- Parent lookup needs a helper to return either hashed base genes or sidecar
  spread genes.

This saves memory, but it creates special-case logic. Since this is a personal
in-development game and storage size is not the main constraint, this option is
not worth the extra complexity for the first version.

### Option C: Lineage Table

Store a small `lineage_id` per plant and keep genes in a lineage table:

```rust
plant.lineage_id -> lineage genes, parent ids, generation, stats
```

Pros:

- Great for ancestry, debug stories, and memory sharing among siblings.
- Can support named emergent populations later.

Cons:

- More moving parts: lineage creation, pruning, save/load, id stability.
- A mutation in one child usually creates a new lineage anyway.
- Harder to make deterministic and compact in the first version.

This is attractive later, but too much for the first playable loop.

### Decision

Use Option A: store genes directly on every `Plant`, including generated base
flora. Keep the biology model simple even if the binary snapshot grows.

## Save Format Plan

Current spread save writes only spread plants with `SPREAD_VERSION = 1` and
`PLANT_BYTES = 14`. Base snapshots also store compact plant attributes.

For evolution:

- Add eight gene bytes to the packed `Plant` representation.
- Bump `SPREAD_VERSION`.
- Bump the base snapshot version.
- Regenerate `world_base.bin`.
- Reject older `plants.bin` and `world_base.bin` files as incompatible.

No migration is needed. This is a personal game in active development, so the
simplest approach is to invalidate old plant saves and start a fresh
evolution-capable population.

### Exact Storage Touchpoints

Update these runtime/storage paths together:

- `Plant` in `src/world_runtime/plant_world.rs`
  - add `genes: PlantGenes`
  - packed runtime size becomes about 24 bytes
- `write_plant()` / `read_plant()`
  - append/read eight gene bytes after `born_hour`
  - bump `PLANT_BYTES` from `14` to `22`
  - bump `SPREAD_VERSION`
- Base snapshot encoding in `serialize_base()`
  - `attr` currently writes `height`, `rotation`, `species`
  - extend it to write `height`, `rotation`, `species`, eight gene bytes
  - bump `BASE_VERSION`
- Base snapshot decoding in `parse_base_chunk()`
  - read the extended attr record
  - construct `Plant { genes, .. }`
  - update attr section bounds from `count * 3` to `count * 11`
- Base generation in `Plant::pack()` / `generate_base()`
  - compute initial genes when packing generated base plants
  - generated base plants are mature as before, but now carry real genes
- Spread birth in `land_chunk_candidates()`
  - accepted seedlings store inherited/mutated genes directly on `Plant`
- Tests near plant save/load and base snapshot parsing
  - update expected record sizes and round-trip assertions

Because old snapshots are invalidated, there should be no compatibility branch
for older `BASE_VERSION` or `SPREAD_VERSION`.

## Base Flora Plan

Base flora should be completely similar to later flora. The only difference is
where it comes from:

- base flora is generated at world creation
- later flora is born during spread

After creation, there should be no ecological or behavioral distinction. Base
plants should:

- carry stored genes
- reproduce through the same rules
- compete through the same rules
- express phenotype through the same mapping
- be inspectable through the same tools
- influence later generations exactly like spread-born plants

Base generation should assign initial genes using a simple deterministic founder
function. The generated genes can still be biased around the local environment so
the initial world starts plausible, but the result is stored on the plant rather
than recomputed later.

Suggested base-founder strategy:

```text
wet_pref ~= local moisture + jitter
alt_pref ~= normalized altitude + jitter
other genes random around species defaults
```

### Initial Gene Generation Formula

Use deterministic environment-biased founders:

```text
wet_pref = clamp01(local_moisture + jitter(seed, plant, wet) * 0.25)
alt_pref = clamp01(normalized_altitude + jitter(seed, plant, alt) * 0.25)
stress_width = random_range(0.35, 0.75)
capture = random_range(0.35, 0.75)
fecundity = random_range(0.35, 0.75)
seed_mass = random_range(0.35, 0.75)
dispersal = random_range(0.35, 0.75)
timing = random_range(0.35, 0.75)
```

Where `jitter` is a deterministic signed value in `-1..1` keyed by world seed,
canonical chunk, local position, species, and gene salt.

This starts the world plausibly adapted without making every founder identical.
The random ranges are intentionally moderate so early populations have room to
move under selection.

## Phenotype Evaluation

Add pure functions in `world_core::evolution`:

```rust
pub fn genes_from_bytes(bytes: [u8; 8]) -> PlantGenes;
pub fn initial_genes(seed: u32, key: FounderKey, env: PlantEnvironment) -> PlantGenes;
pub fn inherit_and_mutate(parent: PlantGenes, seed: u32, key: MutationKey) -> PlantGenes;
pub fn evaluate_phenotype(
    genes: PlantGenes,
    species: &PlantSpeciesInfo,
    env: PlantEnvironment,
) -> PlantPhenotype;
```

Keep these pure and deterministic. `world_runtime::plant_world` should provide
the sampled environment and stable hash keys, but the biology math should live in
`world_core`.

## Environment Sampling

The first version needs enough environmental inputs to drive selection:

- moisture from `Heightmap::sample_moisture`
- altitude from `Heightmap::sample_height`, normalized against world/biome range
- river wetness from `RiverField::wetness`
- local competition from nearby plants in the landing chunk, then later from
  loaded/rendered plants if needed

`land_chunk_candidates()` already samples height, moisture, river wetness, biome,
houses, slope, and spacing. It is the right first integration point for
environment and establishment fitness.

For mature plant rendering and lifecycle, `PlantWorld::instances_for()` already
reconstructs plants against a chunk terrain. It can compute phenotype for visible
plants and fill extra fields on `PlantInstance`.

### Altitude Normalization

Use a stable world-level normalization, not per-chunk min/max:

```text
normalized_altitude = clamp01((height - sea_level) / (height_max - sea_level))
```

For the first implementation, `height_max` should come from configuration if a
clear maximum exists; otherwise define a constant in `world_core::evolution`,
for example `EVOLUTION_ALTITUDE_MAX = 220.0`. The important point is that an
`alt_pref` byte has the same meaning everywhere and after reload.

If the terrain configuration later exposes a more precise expected maximum, move
the normalization to that value and bump evolution tuning deliberately.

## Competition Plan

First version competition should be cheap and local.

During landing:

1. Build the existing `SpacingGrid`.
2. Add a second coarse competition grid/query that scans nearby cells.
3. Store enough neighbour data for ecology, not just spacing:
   - local position
   - species
   - growth stage
   - height
   - genes
   - optional cached `capture`, `wet_pref`, and `stress_width` as unit floats
4. For each neighbour, estimate:
   - canopy pressure from neighbour height/capture/growth stage
   - root pressure from distance and wet-pref similarity
   - similarity penalty from phenotype or gene similarity
5. Produce `competition_room` in `0..1`.
6. Multiply candidate establishment chance by `competition_room`.

This makes competition matter at seed establishment, which is the cheapest place
to apply it. Later, mature plants can have growth/lifespan stress affected by
neighbours too.

Important first-version rule:

```text
similar neighbours suppress each other more than dissimilar neighbours
```

That is the mechanism that can produce new niches instead of one local optimum.

### First Competition Formula

Keep the first formula deliberately simple:

```text
distance_falloff = smoothstep(radius, 0, distance)
stage_weight = seedling 0.15, young 0.45, mature 1.0, dead 0.05
canopy = distance_falloff * stage_weight * neighbour_capture * neighbour_height_scale
wet_similarity = 1 - abs(candidate_wet_pref - neighbour_wet_pref)
root = distance_falloff * stage_weight * wet_similarity

pressure = canopy * 0.55 + root * 0.45
competition_room = clamp01(1 - accumulated_pressure * COMPETITION_STRENGTH)
```

Use this only during candidate landing at first. Mature plant phenotype can use
`shade = 0` and `root_pressure = 0` until lifecycle/growth competition is wired.

## Spread And Inheritance Plan

Current spread logic:

- each mature plant rolls against `species.placement.spread_chance`
- each successful plant emits 1-2 seed candidates
- each candidate lands if terrain/spacing checks pass

Evolution changes this to:

1. Look up parent genes.
2. Evaluate parent phenotype at its current environment.
3. Scale spread chance by parent fitness and fecundity.
4. Scale seed count by fecundity, seed mass, capture, and stress.
5. Scale spread radius by dispersal and seed mass.
6. For each candidate, inherit and mutate parent genes.
7. Evaluate candidate establishment at target environment.
8. Roll establishment chance before accepting.
9. Store candidate genes directly on the accepted `Plant`.

Initial implementation can keep the existing 1-2 seed count and use the
phenotype as a probability multiplier. Once stable, switch to variable seed
count.

Mutation speed should not be a user-facing evolution-speed setting. Evolution
should become faster or slower mainly because simulation time passes faster or
slower through the existing time/day-speed controls. Mutation chance and mutation
step can exist as internal biological constants, but the player-facing control
is time.

## Lifecycle Plan

`life_schedule()` currently uses species placement timings plus deterministic
jitter.

Evolution should affect:

- seedling/young duration through `timing`, `capture`, and stress
- mature lifespan through `timing`, `abiotic_fitness`, and possibly competition
- visual stress through phenotype, not lifecycle stage

Two technical options:

### Evaluate Schedule Dynamically

Call `evaluate_phenotype()` inside `life_schedule()`.

Pros:

- Plants living in bad places die sooner and mature differently.

Cons:

- `life_schedule()` currently does not have environment access.
- Schedule can change if environmental calculations change.
- More invasive to `tick_growth()`.

### First Version: Keep Schedule Mostly Species-Based

Use genes only to scale spread, establishment, and render phenotype first. Add
lifespan/maturity scaling in a second patch after environment plumbing is in
place.

Pros:

- Smaller first slice.
- Evolution is already visible through reproduction, establishment, height, and
  color.

Cons:

- Timing gene is less meaningful until lifecycle scaling lands.

Recommended: implement lifecycle scaling after the genotype/storage/spread/render
loop works. Keep the plan's first-version scope, but stage it late.

## Rendering Plan

`PlantInstance` currently carries:

```rust
position, rotation, height, species_index, growth_stage, decay
```

Add phenotype-derived render fields:

```rust
height_scale: f32
width_scale: f32
stress: f32
leaf_tint: [f32; 4]
```

Then `build_plant_instances()` can apply:

```text
scale.xz *= width_scale
scale.y *= height_scale
color *= leaf_tint
```

Technical caveat: for full procedural tree meshes, current living trees use
white instance tint because mesh colors are baked. If leaf stress needs to tint
only leaves, instance color may tint trunk and leaves together. First version can
accept whole-plant stress tint, or use it only for shrubs/billboards and debug
overlays. A later renderer pass can split bark/leaf material tint.

Recommended first render path:

- Apply height/width scale to every plant.
- Apply stress tint to shrubs immediately.
- For tree meshes, use only subtle whole-instance stress tint in natural
  rendering.
- Use strong gene/fitness colors in the evolution overlay until leaf-only tree
  tint exists.

## Evolution Overlay Plan

The first visible tool should be simple:

- Add a debug/evolution overlay mode enum:
  - off
  - wet preference
  - altitude preference
  - abiotic fitness
  - competition stress
  - generation
- Store the selected mode in app state.
- Fill `PlantInstance.leaf_tint` or `debug_tint` from the selected mode.
- Expose it both through the debug API and an in-game keyboard/UI toggle.

This is better than waiting for a polished UI. The feature needs immediate
visual feedback.

### First Control Path

Use both:

- Keyboard: cycle overlay mode with a single key. Prefer `E` for evolution if it
  is free; otherwise use a function key or add it to the existing debug/UI key
  pattern.
- Debug API: add `set_evolution_overlay { mode }` so screenshots and automated
  checks can force a known mode.

In-game UI can follow later. The first implementation only needs a reliable
keyboard cycle and debug command.

## Region Readout Plan

Add a debug command, for example:

```json
{ "type": "inspect_evolution_region", "x": 1200, "z": 900, "radius": 128 }
```

Return:

- plant count
- mean and standard deviation for each gene
- mean phenotype values
- mean abiotic fitness
- mean competition stress
- seedling/young/mature/dead counts
- optional top species counts

Implementation can scan canonical chunks overlapping the radius. This is not per
frame, so a simple scan is fine.

The existing `CommandAppliedEvent.data` field can carry the JSON payload without
changing the WebSocket event model.

## Technical Sequence

These phases are the implementation phases for the first version. Phase 7 is not
optional polish: lifecycle scaling is part of the first-version promise because
the design says selection can act through growth and lifespan. It is staged last
only to reduce risk after storage, inheritance, selection, rendering, and debug
observability are working.

### Phase 1: Pure Evolution Math

- Add `world_core::evolution`.
- Define genes, environment, phenotype, conversion helpers.
- Implement trade-off mapping.
- Unit-test:
  - every gene has at least one measurable upside and downside
  - specialist beats generalist at ideal site
  - generalist beats specialist off-niche
  - high seed mass improves establishment but reduces seed count
  - high dispersal increases radius but reduces establishment

### Phase 2: Storage And Gene Lookup

- Add genes directly to `Plant`.
- Add helpers:
  - `genes_for(plant)`
  - `push_plant(...)`
  - `initial_genes(...)` during base generation
- Bump spread save format and base snapshot format.
- Regenerate base snapshots.
- Unit-test base snapshot and spread save/load round trips with genes.

Files/functions expected to change:

- `src/world_core/mod.rs`
- `src/world_core/evolution.rs`
- `src/world_runtime/plant_world.rs`
- `Plant`
- `Plant::pack`
- `serialize_base`
- `parse_base_chunk`
- `write_plant`
- `read_plant`
- `SPREAD_VERSION`
- `BASE_VERSION`
- `PLANT_BYTES`

### Phase 3: Inheritance And Mutation

- Thread parent genes through `SpreadCandidate`.
- Add child genes to accepted candidates.
- Implement deterministic mutation from candidate order/bucket/position.
- Unit-test:
  - same world seed and pass produce identical genes
  - mutation clamps to `0..=255`
  - child genes remain near parent genes with the default mutation constants

Implementation notes:

- `SpreadCandidate` should carry child genes once emitted.
- Parent genes are read directly from `Plant`.
- Mutation is deterministic from world seed, canonical chunk, parent index,
  spread bucket, and seed index.
- Mutation constants are internal biology/tuning constants, not user-facing
  speed controls.

### Phase 4: Selection In Spread Landing

- Evaluate parent phenotype for spread chance/seed count/radius.
- Evaluate candidate phenotype for establishment.
- Add local competition pressure.
- Unit-test:
  - bad moisture/altitude sites reject more candidates
  - high competition reduces establishment
  - dissimilar neighbours suppress less than similar neighbours

Implementation notes:

- Parent phenotype affects spread roll, seed count, and spread radius.
- Candidate phenotype affects establishment.
- Competition is evaluated during landing only in this phase.
- The existing terrain/biome/houses/slope hard rejects should remain as hard
  ecological constraints; phenotype-based establishment is an additional
  probabilistic gate.

### Phase 5: Render Phenotype

- Extend `PlantInstance` with render phenotype fields.
- Compute visible phenotype in `instances_for()`.
- Apply scale/tint in `build_plant_instances()`.
- Screenshot-check at high day speed with the debug CLI.

Files/functions expected to change:

- `src/world_core/chunk.rs` (`PlantInstance`)
- `src/world_runtime/plant_world.rs` (`instances_for`)
- `src/renderer_wgpu/instancing.rs` (`build_plant_instances`)
- shader changes only if current instance color/scale channels are insufficient

### Phase 6: Debug Observability

- Add region inspector debug command.
- Add telemetry summary fields only if cheap; otherwise keep it command-based.
- Add first evolution overlay mode.
- Use screenshots to compare normal rendering and overlay rendering.

Files/functions expected to change:

- `src/debug_api/types.rs`
- `src/app/debug_commands.rs`
- `tools/debug-cli/cli.ts`
- app state for overlay mode
- render/update path that passes overlay mode into plant instance construction

### Phase 7: Lifecycle Scaling

- Thread enough environment into lifecycle evaluation to scale maturity/lifespan.
- Keep analytic determinism: a reloaded world must transition stages at the same
  sim-hour as a live world.
- Unit-test stage transitions with gene-scaled schedules.

Implementation notes:

- `life_schedule()` needs access to plant genes and a deterministic environment
  sample.
- Avoid dependence on loaded chunk terrain or per-frame state. Use retained
  `Heightmap`/`RiverField` sampling so loaded and reloaded worlds agree.
- Cache nothing that can diverge from save/load unless it is purely derived.
- After this phase, `timing`, `capture`, `abiotic_fitness`, and stress should
  influence at least maturity timing or lifespan.

## Risks And Mitigations

| Risk | Mitigation |
|---|---|
| Evolution is mathematically real but visually invisible | Ship overlay and region inspector in the first version |
| One gene becomes universally optimal | Unit-test trade-offs and keep the "no purely good gene" rule in code review |
| Competition makes spread too expensive | Reuse/coarsen `SpacingGrid`; apply competition only during landing first |
| Base founder genes look chaotic | Bias initial stored genes around local environment |
| Save compatibility becomes annoying | Do not migrate old saves; version formats and regenerate |
| Tree stress tint looks wrong because color is baked | Tint shrubs normally; keep strong colors overlay-only for trees |

## Decisions

- Keep the simple model: genes are stored directly on every plant.
- Invalidate old plant saves and base snapshots; no migration code.
- Base flora is only the initial population, not a special population.
- Use existing time/day-speed controls to make evolution happen faster or slower.
- Provide the first evolution overlay through both debug API and in-game controls.
- Tint shrubs naturally in v1; keep strong tree colors for overlays until
  leaf-only tinting exists.
