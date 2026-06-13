# Plant Evolution — System Design

> Status: design proposal. This document describes the *game system* for plant
> genetics and evolution, not the implementation. It defines the genes, how they
> become visible traits, how they're inherited and mutate, and how the
> environment selects between them. Implementation details (struct layouts,
> call-sites, serialization) come later, once the design is agreed.

## Goal

Let plants slowly diverge as they spread across the world: lineages that drift
toward the conditions they live in, so that — over many in-game days — wetlands,
dry ridges, and high slopes come to hold visibly different-looking plants of the
same species. It should feel like adaptation, not like a random skin shuffle.

The hard constraint is that **this is a game, not a biology simulator**. The gene
set is deliberately tiny and abstract. Richness comes from *combination* and from
*genes meeting the environment*, not from a large catalogue of authored variants.

## The one idea that everything rests on: genotype ≠ phenotype

We keep two layers strictly separate.

- **Genotype** — a short vector of abstract genes the plant carries. Heritable,
  mutable, and *not* directly meaningful on screen. A gene is never "the height
  slider"; it is a tendency.
- **Phenotype** — what you actually see and measure: rendered height, width,
  leaf color, how long it lives, how often it reproduces. The phenotype is
  **computed** from the genotype **and the local environment**.

The function that maps `(genotype, environment) → phenotype` is where the whole
game lives. Two rules make it interesting rather than a set of renamed sliders:

1. **One gene can fan out to many traits** (pleiotropy). The growth gene drives
   both height and girth, so a mutant looks like a *plausible* bigger plant, not
   a stretched one.
2. **One trait can be driven by many genes.** Final height = growth gene *scaled
   by* how well the tolerance genes match this spot. A plant that is genetically
   vigorous but living somewhere it hates stays stunted.

Because the mapping is many-to-many, we never author "tall swamp variant" — we
author a few genes and one mapping function, and the player *discovers* the
combinations that emerge across the terrain.

## The gene set (genotype)

Four genes to start. All are abstract scalars in `[0, 1]`; none renders anything
on its own.

| Gene | What it represents | Role in the mapping |
|---|---|---|
| **Vigor** | how much the plant invests in raw size | The pure size driver: scales height *and* width together, and slightly speeds maturation. The only gene that touches size directly. |
| **Wetness preference** | the moisture level this plant is happiest at (0 = dry-loving … 1 = swamp-loving) | An *optimum*, not a knob. Compared against local moisture to produce a fitness value. Renders nothing by itself. |
| **Altitude preference** | the elevation / "air density" band it favors (0 = lowland … 1 = highland) | A second optimum, compared against local altitude. Lets lineages climb or stay low. |
| **Hardiness** | how wide a range it tolerates around its preferences | Widens or narrows the fitness response. High hardiness = generalist that does okay everywhere; low hardiness = specialist that's excellent in its niche and poor outside it. |

Three of these (wetness, altitude, hardiness) feed a single combined **fitness**
number; vigor feeds size directly. That keeps the design legible: *one gene for
"how big," two genes for "where do I belong," one gene for "how picky am I."*

> The altitude gene is the newly requested axis. Framing it as "air-density / how
> high it likes to grow" gives a second independent environmental gradient, so
> populations can specialize along *two* directions at once (a wet-lowland type
> and a dry-highland type can coexist and diverge separately).

## How genotype becomes phenotype

A single conceptual function, applied everywhere a plant's traits are needed:

```
# 1. How well does this genotype match where it actually is?
wet_match = bell(local_moisture; center = wetness_pref, width = hardiness)
alt_match = bell(local_altitude; center = altitude_pref, width = hardiness)
fitness   = combine(wet_match, alt_match)      # 0 = wrong place, 1 = ideal spot

# 2. Visible / measurable traits derive from genes AND fitness
realized_height = base_height × vigor × (floor + (1-floor) × fitness)
realized_width  = base_width  × vigor × (floor + (1-floor) × fitness)
lifespan        = base_lifespan        × fitness
spread_chance   = base_spread_chance   × fitness
leaf_color      = base_color  shifted by stress (low fitness → paler/duller)
```

Key consequences for the feel of the game:

- A **swamp-preference** plant that sprouts on a **dry ridge** grows stunted,
  pales, lives briefly, and rarely seeds — so its offspring lose ground there to
  better-matched neighbors. The same genotype in a wetland thrives. *That* is
  adaptation, and it falls out of the mapping rather than being scripted.
- **No gene is a direct visual control.** The visible difference (lush vs.
  stunted, green vs. pale) emerges from genotype meeting environment. This is the
  genotype/phenotype separation working as intended.
- The `base_*` values stay exactly the species presets we already have. Genes are
  *modifiers on top of* the authored baseline, so an unmutated plant looks like
  today's plant.

## Inheritance, mutation, drift

- **Inheritance.** A seedling copies its parent's genotype.
- **Mutation.** At birth, each gene gets a small random nudge, clamped to
  `[0, 1]`. Mutation size is a single global tuning value (call it the
  *mutation rate*) so we can dial evolution's speed up for demos and down for
  realism.
- **Drift vs. selection.** Mutation alone is aimless drift. Selection comes from
  the fitness coupling above: better-matched plants live longer and reproduce
  more, so their (similarly-matched, slightly-mutated) offspring come to
  dominate a locale. Over many generations a population *clines* along the
  moisture and altitude gradients.
- **Optional later: gene flow / pollination.** When a seedling lands, nudge its
  genotype slightly toward nearby mature plants of the same species. This makes
  regional varieties converge into recognizable local "breeds" instead of every
  lineage drifting in isolation. Out of scope for the first version.

## Two populations, two policies

The world has two distinct plant populations, and evolution applies differently
to each — this keeps storage and determinism intact.

- **Base flora** (the world as first generated). Huge, regenerated from seed, not
  individually stored. Its genotypes are derived from a **position + seed hash**,
  so founders start already roughly matched to their biome and cost *nothing* to
  store. Base flora is treated as a fixed **founder stock**: it does not itself
  mutate or change over time. The fitness mapping still applies, so even founders
  vary in size/color by how well their hashed genes suit their spot.
- **Spreading population** (everything born from reproduction during play). This
  *is* the evolving set. It carries real genotypes that inherit and mutate, and
  it's the population that genuinely adapts over in-game time. It's already the
  population we persist separately, so confining evolutionary state to it means
  the static world snapshot stays static and reload-deterministic.

Net effect: the bulk of the world is a stable, reproducible backdrop, and a
living, adapting frontier grows out of it.

## Making evolution *visible* (it's a game)

Real selection is slow; a player must be able to see it.

- **Lead with the loud traits.** Size (vigor × fitness) and leaf color (stress
  tint) are the immediately legible signals. Flying over a moisture or altitude
  gradient should show a smooth cline of plant size/color — that reads instantly
  as "these adapted to here."
- **Stress coloring is optional pleiotropy worth taking.** Letting low fitness
  drift leaves toward pale/dull (and ideal fitness toward rich green) makes
  adaptation readable at a glance, not only by comparing heights. Cheap, because
  per-instance color already exists.
- **Tunable drama.** Mutation rate and selection strength are global dials, so
  evolution can be sped up for showing off and slowed for a more naturalistic
  pace.
- **A readout.** Some way to inspect a region's gene distribution (e.g. mean
  wetness-preference, mean vigor over an area) so slow drift is observable and
  debuggable — otherwise the system's most interesting behavior is invisible.

## Scope: first version vs. later

**First version**
- Four genes: vigor, wetness preference, altitude preference, hardiness.
- One genotype→phenotype function with the combined fitness term.
- Traits driven: rendered height, rendered width, lifespan, spread chance, and
  (recommended) stress-based leaf tint.
- Inheritance + mutation on the spreading population; hashed founder genes on
  base flora.
- A region-level gene readout for observation/tuning.

**Later**
- Gene flow / pollination toward local neighbors.
- Genes that affect *shape*, not just size — branchiness, crown form. These
  change geometry, so they'd be quantized into a few discrete per-species
  "morphs" rather than continuous, to avoid giving every plant a unique mesh.
- More environmental axes (slope, temperature) if the two-gradient model proves
  too thin.

## Design principles to hold onto

1. **Genes are abstract; traits are computed.** Never collapse a gene into a
   direct visual slider — that throws away the genotype/phenotype distinction
   that makes combinations emergent.
2. **Few genes, rich combinations.** Resist adding genes to get variety; get
   variety from the mapping and the environment instead.
3. **Environment is half the equation.** A genotype has no fixed phenotype — only
   a phenotype *in a place*. This is what makes plants appear to adapt.
4. **Keep the static world static.** Evolution lives in the spreading population;
   the base world stays a reproducible backdrop.
5. **Visible beats accurate.** When realism and legibility conflict, favor the
   version the player can see.
