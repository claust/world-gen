# Plant Death & Decay — Feature Design

Status: **implemented** (see "Implementation notes" at the end for deviations)
Date: 2026-06-09

## Summary

Plants currently grow through three stages (`Seedling → Young → Mature`) and then
live forever. Once a chunk saturates, the ecosystem freezes: no plant ever makes
room for another. This feature adds the missing end of the life cycle:

> After a plant has matured and seeded, it **dies** — swapping to a leafless,
> crooked, slightly smaller "snag" model — stands for a while as dead wood, and
> then **despawns**, freeing its ground and re-opening the chunk so new
> seedlings can claim the space.

The result is a forest that *turns over*: old giants fall, gaps open, seedlings
race in. The world stays alive instead of settling into a static climax state.

## Decisions made

| Question | Decision |
|---|---|
| Stage shape | Single new stage: `Mature → Dead → despawn` (no separate "Declining" stage in v1) |
| Who dies | **Everything** — base-world plants get a synthetic age so the starting forest turns over gradually |
| Timescale | Short snag: lifespan ~600–1000 sim-hours mature, snag stands ~100–150 h, ±25 % per-plant jitter |
| Visual extras (v1) | Bark color shift (grey/brown tint), slight random lean, shrink-on-decay (1.0 → ~0.8 over the snag phase) |

## Current state (what we build on)

- `GrowthStage` enum in `src/world_core/lifecycle.rs` — `Seedling`, `Young`,
  `Mature`, with hardcoded `scale_factor()` (0.15 / 0.50 / 1.0).
- Stages are **analytic**: a plant stores only `born_hour`; `stage_for()` in
  `src/world_runtime/plant_world.rs` derives the stage from age vs. the
  species' `seedling_hours` / `young_hours` thresholds. No per-plant timers.
- One procedural mesh per species (`plant_gen::generate_plant_mesh`); stages
  differ only by uniform scale and LOD bucket. Foliage is a list of
  `FoliageBlob` SDF spheres surfaced by marching cubes on top of bark cylinders.
- Only `Mature` plants spread. A chunk is flagged *saturated* when it has no
  immature plants and the last spread pass added nothing; saturated
  neighbourhoods skip the spread pass entirely.
- Persistence: base plants live in `world_base.bin` (implicitly mature);
  runtime-spread plants are an append-only delta blob (14 bytes/plant,
  includes `born_hour` and cached `stage`).

## Design

### 1. The `Dead` stage

```rust
pub enum GrowthStage {
    Seedling,
    Young,
    Mature,
    Dead,      // new — leafless snag, does not spread, despawns after snag_hours
}
```

Two new per-species `PlacementConfig` fields (serde defaults so existing JSON
deserializes):

- `lifespan_hours` — time spent **Mature** before dying (measured from the
  moment the plant becomes mature).
- `snag_hours` — time the dead snag stands before despawning.

The stage stays a pure function of age — death and despawn are just two more
thresholds in `stage_for()`:

```
young_at   = seedling_hours
mature_at  = young_at + young_hours
dead_at    = mature_at + lifespan_hours * jitter(plant)
gone_at    = dead_at   + snag_hours    * jitter2(plant)
```

`jitter()` is a deterministic hash of the plant's identity (canonical chunk +
index + seed, same scheme `spread_roll` uses) mapped to **±25 %**. Without
jitter, every tree planted in the same spread pass — and catastrophically, the
*entire base forest* — would die in the same tick.

Because death is analytic, **despawn needs no tombstones**: on load, any plant
whose `gone_at` is in the past simply never materializes. This keeps the
append-only delta format valid and makes idle worlds catch up for free, exactly
like growth already does.

### 2. Base plants: synthetic age

Base-snapshot plants have no meaningful `born_hour` (they're "implicitly
mature"). To make them mortal without changing the snapshot format, assign each
a deterministic synthetic age at load time:

```
elapsed_in_maturity = hash_unit(plant identity) * lifespan_hours
born_hour           = -(mature_at + elapsed_in_maturity)
```

i.e. each base plant has already lived a random fraction of its mature life
when the world begins. Deaths in the starting forest are then uniformly
staggered across one full lifespan — a steady trickle, never a wavefront.
The hash must be versioned with care: changing it re-rolls every base plant's
death day (acceptable, but should be deliberate).

### 3. Despawn and ecosystem reopening

This is the payoff of the feature and the most state-touching part:

1. **Removal** — `tick_growth()` (or a dedicated reap pass alongside it)
   removes plants past `gone_at` from the chunk's plant list.
2. **Spacing grid** — free the plant's `SpacingGrid` cell so a candidate can
   land there.
3. **Saturation reset** — clear the saturated flag on the chunk **and its 8
   neighbours** (a freed spot near a chunk edge is reachable by neighbours'
   spread radius). Without this, the spread pass keeps skipping the
   neighbourhood and the gap never refills.
4. **Counters** — immature counts are unaffected (dead plants aren't immature),
   but the "last pass added nothing" bookkeeping must be reset along with the
   flag.

Dead snags still **occupy** their spacing cell until despawn — dead wood holds
its ground, which reads naturally and avoids seedlings clipping through snags.

Spread already requires `stage == Mature`, so dead plants stop reproducing with
no extra code.

### 4. The dead snag model

One additional prototype mesh per species, generated through the existing
pipeline from a derived "dead" `SpeciesConfig`:

- `foliage` blob list → **empty** (no SDF pass at all — the snag mesh is just
  bark cylinders, much cheaper than the living mesh).
- Prune ~40 % of branches (drop by seeded roll per branch, biased toward the
  deepest `branching.max_depth` level — twigs rot first).
- Crooked: lower `trunk.straightness` and apical dominance so remaining limbs
  wander; slightly stronger taper for a withered silhouette.
- Slightly smaller: base snag scale ~0.85× of the species' mature scale.

Because the snag mesh has no foliage it's cheap enough to skip a separate LOD
variant in v1 — use the same mesh for both LOD tiers.

Shrubs and cattails get the degenerate version of the same recipe (cattails:
keep a few broken stalks, no seed heads). If a species looks bad dead, a
per-species `dead` override block in its JSON can tune the recipe later.

**Plant editor preview** — the herbarium editor (`src/app/plant_editor.rs`)
currently generates and displays one mesh per species. It should additionally
generate the derived dead config and render the **snag beside the full-grown
plant** (offset on the ground plane, dead tint applied), so the dead look is
visible while designing a species and reacts live to parameter tweaks — both
meshes regenerate through the same `generate_plant_mesh` call on edit.

### 5. Rendering

`build_plant_instances` currently splits per species into (full, LOD) buckets;
add a third **dead** bucket pointing at the snag prototype:

- **Scale (shrink-on-decay)** — interpolate `0.85 → 0.68` (i.e. ×1.0 → ×0.8 of
  snag scale) across the snag phase using `(total_hours - dead_at) / snag_hours`.
  Continuous, uses the existing per-instance scale path.
- **Bark tint** — desaturated grey-brown via the existing per-instance
  `color: [f32; 4]` field; can also darken slightly over the snag phase.
- **Slight lean** — `InstanceData` has only `rotation_y` plus a `_pad: f32`;
  repurpose `_pad` as a tilt angle (radians, hash-derived per plant, ~0–6°)
  and apply it in the instanced vertex shader. Zero for living plants, so the
  change is backwards-compatible.

### 6. Persistence & gen_key

- **Delta blob** — no format change. `stage` byte gains value 3 (`Dead`);
  despawned plants can be pruned from the blob on save as a size optimization,
  but correctness doesn't require it (they're filtered analytically on load).
- **Base snapshot** — no format change; synthetic age is computed at load.
- **gen_key** — `Herbarium::generation_key` serializes the *whole*
  `PlacementConfig` per species (`BaseGenerationPlant` in
  `src/world_core/herbarium.rs`), so adding `lifespan_hours` / `snag_hours`
  changes the key and invalidates the existing `world_base.bin`. That's fine —
  the sole player can regenerate; no key-view or other precaution needed.
  Just regenerate the base (and `--generate-base-web` for the web bundle,
  keeping any defaults that feed the key mirrored between native and wasm).

### 7. Proposed default numbers (tune later)

Per-plant jitter: ±25 % on both `lifespan_hours` and `snag_hours`.

| Species class | `lifespan_hours` (mature) | `snag_hours` | Full cycle (mean) |
|---|---|---|---|
| Trees (oak, …) | 720 (30 sim-days) | 120 | ~36 sim-days sprout-to-gone |
| Shrubs | 360 | 48 | ~19 sim-days |
| Aquatics (cattail) | immortal (`0`) | — | base-only species never die |

Rationale: with default `seedling_hours`=48 / `young_hours`=96, a tree spends
~80 % of its life mature (plenty of spread passes — every mature tree gets ~30
daily spread rolls at 0.3 chance, so lineage survival is comfortable), and at
high `day_speed` a play session visibly shows turnover. Values live in each
species JSON, so long-lived oaks vs. short-lived shrubs are data tweaks.

### 8. Documentation site updates (`web/`, GitHub Pages)

The biology section currently teaches the three-stage story and must be updated
in the same change:

- **`web/biology-growth.html`** — the main page:
  - Frontmatter `description` / `og_description` say "three-stage life cycle";
    update to four stages and mention death/turnover.
  - Hero stat chip `3 growth stages` → `4 growth stages`.
  - `#stages` section: add **Dead** to the stage list, scale story
    (0.15 / 0.50 / 1.0 / ~0.85→0.68), and the timeline SVG — add a DEAD band
    after MATURE and a despawn marker (the SVG is hand-authored; extend the
    `viewBox` bands and labels).
  - Analytic-clock section: extend the "one comparison yields the stage"
    explanation with the two new thresholds, per-plant jitter, and the
    no-tombstone despawn argument (it's the same elegant trick, worth telling).
  - `#spread` section: add a short "Turnover" passage — death un-saturates
    chunks, so settled forests reopen; the ecosystem reaches a dynamic
    equilibrium instead of a frozen one.
- **`web/biology.html`** — overview page references the life cycle; update its
  teaser copy for the growth subpage.
- **`web/biology-evolution.html`** — mentions stages/maturity; check that
  selection copy still reads correctly when individuals die (it likely gets
  *stronger*: death is what makes generational turnover real).
- **`web/concept-instanced-rendering.html`** — mentions the seedling/young LOD
  split; add a line about the third (dead snag) bucket.
- Verify locally with the Jekyll build + headless-Chrome recipe (see
  docs-site memory) before pushing.

## Implementation phases

1. **Core death** — `Dead` enum variant, `lifespan_hours`/`snag_hours` config
   (gen_key changes; regenerate `world_base.bin`), extended `stage_for()`,
   jitter, base-plant synthetic age, despawn/reap pass, spacing-grid free +
   saturation reset.
   Render dead plants with the existing mesh (tinted) just to prove the loop.
2. **Snag meshes** — dead `SpeciesConfig` derivation (foliage strip, branch
   prune, crook), third instance bucket, per-species prototypes; plant-editor
   side-by-side snag preview.
3. **Visual polish** — bark tint curve, lean via `_pad` tilt in the shader,
   shrink-on-decay interpolation.
4. **Docs site** — `web/biology-*.html` updates per §8.
5. **Tuning pass** — per-species numbers, watch a high-`day_speed` world for
   equilibrium (forest should neither vanish nor stay frozen).

## Testing & verification

- **Unit**: `stage_for()` threshold/jitter cases; despawn determinism
  (save → load → identical plant set); saturation reset on reap.
- **Sim**: headless fast-forward N sim-days, assert population stays within a
  band (no extinction, no unbounded growth) and that dead/despawned counts are
  nonzero.
- **Visual** (debug CLI loop): `set_day_speed` high → screenshot a known dense
  area over several sim-days → verify snags appear (leafless, tinted, leaning),
  then disappear, then seedlings refill the gaps. The river overlook camera in
  project memory is a good fixed vantage point.

## Out of scope (future ideas)

- A `Declining` stage (sparse brown foliage) between Mature and Dead.
- Falling animation / fallen logs / stumps on despawn.
- Nutrient patches: temporary local spread-chance boost where a tree despawned.
- Death from causes other than age (drought via moisture, crowding, lightning).
- Audio (creaks, falling crash).

## Implementation notes (deviations from the design above)

- **Cattails are immortal**, not 240/24 as first proposed: they are base-only
  (`spread_chance 0`), so death would permanently empty every reed bed.
  `lifespan_hours <= 0` is the immortal sentinel. Giving aquatics real spread
  (with an aquatic-aware landing guard) is future work.
- **Chunk skip generalized**: the growth tick's old "skip when no immature
  plants" filter would have frozen mature plants short of death. Each chunk now
  tracks `next_event` — the earliest sim-hour any of its plants crosses a life
  threshold — and is skipped until then. `0.0` forces a recompute (used at
  construction/load and when spread adds seedlings).
- **Base synthetic age** is implemented without touching `born_hour` (kept at
  0): base plants are identified positionally (index < `base_count`) and draw a
  uniform-fraction lifespan instead of the spread plants' ±25% jitter.
- **Dead shrubs reuse the billboard** under the shared `DEAD_TINT` rather than
  getting a bespoke snag card — at shrub scale the tint reads fine.
- **Lean** rides in the `InstanceData` field formerly named `_pad` (now
  `tilt`), exposed to the instanced + shadow shaders as vertex attribute 7.
- **Decay shrink** updates whenever the chunk's instances are rebuilt (any
  global sim change refreshes loaded chunks), giving a stepwise but visually
  smooth 0.85→0.68 ease over the snag phase.
