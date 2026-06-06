# Cattails (aquatic reeds)

A new vegetation species: tall, thin **cattails** that grow in clumps at the
edge of rivers and lakes and a short way **into the shallow water**. Each
plant instance renders as a small bunch of slender green stalks, most of them
topped with the iconic brown sausage-shaped seed head. Beds of clumps line the
banks, so the waterline reads as a living margin rather than a hard cut.

This is the first **aquatic** species. It is also the first species that is
*meant* to sit in the water, which puts it in direct tension with the placement
rule added in #103 (see below) — resolving that tension cleanly is the core of
the design.

## What's already in place (post-#103)

PR #102 (river water surface) and #103 (keep plants out of channels) did most
of the plumbing we would otherwise have had to add:

- **River wetness reaches placement.** `terrain.river` (per-vertex `0..1`
  wetness) is already sampled inside `FloraLayer::place_grid`
  (`world_core/content/flora.rs`). We do **not** need to thread river data into
  `FloraInput` — it's there.
- **A shared pivot constant exists.** `MAX_PLANTABLE_WETNESS = 0.15`
  (`world_core/rivers.rs`) is the agreed boundary between "plantable bank" and
  "in the river." Land plants are rejected *above* it; cattails will be the
  mirror image — spawned *above* it. The two species sets tile against this same
  line with no overlap and no gap.
- **Base gen key is at v3.** Adding a species changes the baked plant set, so we
  bump **v3 → v4** (`world_core/herbarium.rs`, `BaseGenerationInputs::version`).
  `world_base.bin` regenerates on next load.
- **The spread/landing pass already holds the `RiverField`** and applies the same
  wetness guard (`world_runtime/plant_world.rs`, `land_chunk_candidates`).

## Three sub-problems

1. **The model** — a new clump-of-stalks generator (new geometry).
2. **Placement** — make the wetness guard *species-aware* so cattails spawn in
   the wet band that everything else avoids, and lift the below-sea-level skip
   for them.
3. **Integration** — preset JSON, registry/placement config, gen-key bump,
   spread policy, verification.

## Decisions locked

- **Look:** cattails — green stalks, brown seed-head spike on most stalks,
  per-clump variation (some bare).
- **Spawn zone:** waterline edge **and** shallow water (partly submerged), not
  edge-only.
- **Spread policy:** **base-only for v1.** Cattails are placed during base world
  generation and do *not* propagate via the lifecycle spread pass. Reed beds
  don't migrate fast, and this keeps cattails out of `land_chunk_candidates`, so
  the second wetness guard there needs no aquatic exemption. Revisit later if we
  want beds to slowly colonise new banks.
- **Build order:** **model-first.** Build the `reed` generator + `cattail.json`
  and iterate it in the plant editor until the clump reads right, *then* wire up
  placement. The model is the riskiest visual piece and the editor gives the
  tightest loop.

---

## The model — options by effort vs. impact

The mesh generator (`world_core/plant_gen/`) builds 8-sided cylinders for
trunk/branches plus volumetric SDF "blob" foliage, with two special paths today:
multi-stem and palm-frond (`plant_gen/tree.rs`).

### Option A — Reuse multi-stem, no new code ⏳ rejected
Set `stem_count` high, `max_depth: 0`, tiny `thickness_ratio`. Produces a bunch
of thin sticks. Cheapest, but reads as bare skewers — no taper character, no
seed head. Useful only as a throwaway sanity check.

### Option B — New `body_plan.kind: "reed"` generator ✅ chosen
A dedicated path alongside the palm-frond special case in `plant_gen/tree.rs`.
For one instance:

- Emit **N stalks** (≈ `stem_count`, e.g. 5–11) fanning from a small shared
  base, each a tall, very thin, gently curved tapered cylinder with mild
  per-stalk lean/azimuth jitter so the bunch looks natural.
- On ~60–70% of stalks, add a **brown cattail spike** near the tip: a short,
  fatter tapered cylinder in a second color. The rest stay bare green.
- Per-clump seeded variation in stalk count, height, lean, and spike fraction so
  no two clumps are identical.

One `PlantInstance` = one clump, so "several stalks together" is intrinsic to the
model, and beds (next section) stack clumps for the larger grouping. Reuses the
existing instanced render path as `kind: "tree"` (procedural mesh, not the shrub
billboard). Colors ride the existing bark/leaf HSL slots (stalk = leaf green,
spike = bark brown).

### Option C — Flat cross-quad blade primitive ⏳ deferred
True grass-blade geometry (crossed quads) instead of thin cylinders. Best
realism, but a real change to the mesh/foliage system. Not needed for a
convincing cattail; revisit only if cylinders look too round up close.

---

## Placement — species-aware wetness

Today every species hits one global guard in `place_grid`:

```rust
if wetness > MAX_PLANTABLE_WETNESS { continue; }   // reject "in the river"
// ... and earlier: if height < sea_level { continue; }
```

Cattails need both inverted. The plan:

- Add an **`aquatic` flag** (or a wetness band) to `PlacementConfig`
  (`world_core/herbarium.rs`). Land species keep today's behaviour.
- In `place_grid`, branch on it:
  - **land species** → unchanged (skip below sea level, skip `wetness > 0.15`).
  - **cattails** → skip *dry* cells (`wetness < MAX_PLANTABLE_WETNESS`), **allow**
    the below-sea-level band (so they can stand in shallow water), and spawn where
    wetness sits in an aquatic range up toward `1.0`. An upper depth bound (via
    `height` relative to `sea_level`, or a max wetness) keeps them out of the deep
    channel centre so they fringe the water rather than carpet it.
- **Density / clustering** falls out for free: the wet band is a contiguous strip
  along each bank, so spawning on a fine grid inside it naturally produces beds.
  Add per-cell probability for a natural scatter rather than a perfect lattice.
- `MAX_PLANTABLE_WETNESS` stays the single shared boundary: land plants stop where
  cattails begin.

## Integration checklist

- **Preset:** `world_core/plant_gen/species/cattail.json` — `body_plan.kind:
  "reed"`, height ≈ 1.5–2.5 m, tiny thickness, green + brown colors. Register in
  `SPECIES_PRESETS` (`herbarium.rs`).
- **Placement config:** add the cattail rule in `default_placement()` with the
  aquatic flag/band.
- **Gen key:** bump `BaseGenerationInputs::version` **3 → 4** with a comment.
- **Spread:** v1 excludes cattails from spread (base-only). If later included,
  mirror the aquatic exemption into `land_chunk_candidates`.
- **Editor:** the plant editor (`src/ui/plant_editor_panel.rs`) previews the clump
  for iteration; confirm `kind: "reed"` renders there.

## Recommended phasing

1. **Model (Option B)** — `reed` generator + `cattail.json`; iterate in the plant
   editor until the clump + spike reads right.
2. **Placement** — species-aware wetness guard + aquatic band; regenerate the base
   world and screenshot a real river bank.
3. **Polish** — tune density/depth bounds and color, verify banks read as reed
   beds without carpeting open water.

## Open risks / notes

- **Submerged rendering:** cattails stand partly under the translucent river/sea
  water surface. Confirm the instanced pass draws them through the water (depth /
  transparency order) — they should, like terrain does, but verify on screenshot.
- **Depth bound tuning:** the gap between "edge" and "deep channel" is narrow on
  small streams; the upper wetness/depth bound may need per-river-size tuning so
  cattails don't either vanish on thin rivers or fill wide ones.
- **gen_key churn:** this is a second base-world invalidation after #103; fine,
  but worth landing alongside any other pending gen-affecting change to avoid a
  third regen.
