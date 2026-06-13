# Plant Evolution - System Design

> Status: design proposal. This document describes the *game system* for plant
> genetics and evolution, not the implementation. It sketches several possible
> implementation depths, then recommends a first playable path.

## Goal

Let plants evolve in ways the player can watch, understand, and become curious
about. Over many in-game generations, lineages should drift, adapt, compete, and
sometimes split into recognizably different local populations: wetland giants,
dry ridge specialists, fast pioneer shrubs, shade-tolerant understory plants,
and other niches that were not hand-authored one by one.

The system should stay close enough to real biology to feel honest:

- Genes are inherited, mutable information.
- Phenotypes are produced by many genes interacting with the environment.
- Selection acts on survival and reproduction, not on abstract "score" alone.
- Adaptations have costs as well as benefits.
- Populations diverge through selection, drift, gene flow, and competition.

This is still a game, not a research simulator. The design should be compact,
legible, deterministic where needed, and visually rewarding.

## Core Principle: Genotype != Phenotype

Keep two layers strictly separate.

- **Genotype** - compact inherited information. Genes are not direct render
  sliders. A gene should never simply mean "height" or "leaf color".
- **Phenotype** - the realized plant: height, width, color, lifespan, seed
  output, dispersal distance, stress, and competitive strength. Phenotype is
  computed from genotype, species baseline, local environment, and local
  competition.

The central mapping is:

```text
(species baseline, genotype, environment, neighbours) -> phenotype
```

This is where the system becomes alive. One gene may influence several
phenotypes (pleiotropy), and one phenotype should usually depend on several
genes (polygenic traits). A plant is not "tall because it has the tall gene"; it
is tall because its inherited strategy, local fitness, life stage, and
competitive situation allow it to realize height.

## Non-Negotiable Principle: No Gene Is Purely Good

Every adaptive advantage must buy a disadvantage somewhere else.

If a gene can only improve fitness, evolution will simply push it to one end of
its range and the system becomes a hidden upgrade tree. That is not evolution;
it is optimization. Real ecological traits are trade-offs:

- Bigger adults capture more light, but need more water and mature more slowly.
- Heavy seeds establish better, but fewer can be produced.
- Far-dispersing seeds colonize new ground, but establish less reliably.
- Generalists survive many places, but lose to specialists in their ideal niche.
- Fast life cycles reproduce sooner, but shorten lifespan or reduce adult size.
- Shade tolerance helps under a canopy, but limits peak growth in open sun.

This principle should be enforced in the phenotype mapping and in any future
gene additions. A proposed gene is suspect until its cost is clear.

## Existing World Constraints

The current plant architecture has two populations:

- **Base flora** - the huge generated background population. It is derived from
  the world seed and position, and should remain deterministic and cheap to
  reconstruct. It can have hashed founder genotypes, but it should not be the
  main evolving population.
- **Spreading population** - plants born during play. These are already
  persisted separately and participate in lifecycle/spread. This is the natural
  home for inherited genotypes, mutation, recombination, and local adaptation.

That split is useful. The world begins as stable founder stock, then living
evolution happens in the spreading layer on top of it.

## Option 1: Tiny Genes With Real Trade-Offs

This is the most conservative extension of the current plan. Keep a small set
of named ecological genes, but make every one participate in costs.

Possible genes:

| Gene | Meaning | Benefit | Cost |
|---|---|---|---|
| `wet_pref` | Preferred moisture band | Thrives when local moisture matches | Poorer performance away from optimum |
| `alt_pref` | Preferred altitude band | Thrives in matching elevation/temperature | Poorer performance away from optimum |
| `stress_width` | Generalist vs specialist tolerance | Survives broader conditions | Lower peak performance in ideal niche |
| `vigor` | Investment in adult size/capture | Taller, wider, stronger competitor | Slower maturity, higher water demand, fewer seeds |
| `seed_mass` | Investment per seed | Better seedling establishment | Fewer seeds, shorter spread |
| `dispersal` | Colonization strategy | Seeds travel farther | Lower establishment and/or seed mass |

Example mapping:

```text
wet_match = bell(local_moisture, center = wet_pref, width = stress_width)
alt_match = bell(local_altitude, center = alt_pref, width = stress_width)

site_match = combine(wet_match, alt_match)
peak_penalty = lerp(1.15, 0.85, stress_width)  # specialists peak higher
fitness = site_match * peak_penalty

adult_height = species_height * vigor * fitness * water_affordance
maturity_time = species_maturity * lerp(0.8, 1.4, vigor)
seed_count = species_seed_count * (1.0 - seed_mass) * (1.0 - vigor * 0.35)
establishment = fitness * seed_mass * competition_room
spread_radius = species_radius * dispersal * lerp(1.1, 0.7, seed_mass)
```

This option is easy to debug and fits the current runtime well. It should be the
first implementation unless a more complex model is explicitly desired.

## Option 2: Polygenic Phenotypes

This option makes the genetics more biologically honest. Instead of each gene
having a readable ecological name, store a compact vector of abstract genes:

```text
g0..g11 in 0..1
```

Phenotypes are computed from combinations:

```text
wet_pref        = sigmoid(+g0 - g3 + 0.5*g7)
alt_pref        = sigmoid(+g1 + g4 - g8)
vigor           = sigmoid(+g2 + g5 - g9)
stress_width    = sigmoid(+g6 - g0 + g10)
seed_mass       = sigmoid(+g3 + g9 - g11)
leaf_darkness   = species_leaf + stress_shift + 0.2*g4 - 0.1*g8
```

This gives useful biological behavior:

- One mutation can affect several traits.
- Similar phenotypes can arise from different genotypes.
- Hidden genetic diversity can persist before becoming visible.
- Selection acts on expressed phenotypes, not directly on genes.

The downside is observability. Debug tools must show both genotype distribution
and derived phenotype distribution, because a player cannot reason about `g7`
directly. This is a better second step than first step.

## Option 3: Ecological Strategy Genes

This option names genes after ecological strategies rather than visible traits.
It is more readable than abstract genes and richer than simple preference genes.

Possible strategy genes:

| Gene | High value means | Trade-off |
|---|---|---|
| `capture` | Big canopy, strong light competition | Higher water demand, slower maturity |
| `stress_tolerance` | Survives poor sites | Lower peak growth |
| `colonizer` | Many/far seeds, fast expansion | Lower adult competitiveness |
| `seed_investment` | Fewer stronger seedlings | Lower seed count and spread |
| `shade_tolerance` | Establishes under taller plants | Lower maximum growth in full sun |
| `timing` | Fast maturity and turnover | Shorter lifespan or smaller adult form |

These genes can produce recognizable niches:

- **Dry ridge specialist** - high stress tolerance, low capture, low water need.
- **Wetland giant** - high capture, high water demand, poor dry survival.
- **Pioneer shrub** - high colonizer, fast timing, low adult dominance.
- **Understory plant** - high shade tolerance, modest height, steady survival.

This option is likely the most joyful to witness because evolution changes plant
roles, not just plant size.

## Option 4: Add Competition As Selection

Terrain selection alone creates smooth clines: plants near water become
wet-adapted, high plants become altitude-adapted, and so on. That is good, but
it will not reliably create rich niches by itself.

Plants should also select against each other.

During seed establishment and possibly growth, compute local competition from
nearby plants:

```text
light_available = reduced by nearby taller/wider adults
root_pressure = reduced by nearby plants with similar moisture strategy
similarity_penalty = stronger when neighbours have similar phenotypes
competition_room = light_available * root_available * spacing_room
```

The important biological rule is:

```text
similar plants compete more strongly than different plants
```

That encourages character displacement. Two lineages in the same area need not
converge on one optimum; one may become taller and water-hungry while another
becomes smaller, faster, or shade-tolerant. This is how new niches can appear.

Implementation can stay cheap. The spread landing pass already validates
spacing. It can also inspect plants in the target chunk and neighbours, compute
a coarse competition penalty, and use that penalty in establishment chance.

## Option 5: Sexual Reproduction And Recombination

Pure parent-copy-plus-mutation works, but it behaves like asexual hill-climbing.
Recombination makes populations feel much more alive.

Simple first model:

```text
mother = plant emitting the seed
father = nearby mature same-species plant, weighted by distance and fitness
child_gene[i] = choose(mother_gene[i], father_gene[i]) + mutation
```

This does not require full diploid genetics at first. Even haploid
recombination gives:

- local populations that blend into recognizable varieties
- hybrid zones between habitats
- useful gene combinations spreading faster
- diversity that selection can recombine instead of waiting for mutation alone

Later, this can grow into diploid alleles with dominance/recessiveness, but that
is not necessary for a first fun version.

## Recommended Path

The first implementation should combine parts of options 1, 3, and 4:

1. Use a compact named genotype with ecological genes.
2. Make every gene participate in at least one trade-off.
3. Compute phenotype from genotype + environment + competition.
4. Let selection act through lifecycle events: growth, survival, seed count,
   seed landing, establishment, and death.
5. Add region-level debug readouts and evolution overlays immediately, because
   invisible evolution will feel like nothing is happening.

Suggested first genotype:

| Gene | Range | Notes |
|---|---:|---|
| `wet_pref` | 0..1 | Moisture optimum |
| `alt_pref` | 0..1 | Altitude/temperature optimum |
| `stress_width` | 0..1 | Generalist/specialist axis; broad tolerance lowers peak fitness |
| `capture` | 0..1 | Adult size and competitive strength; costs water, maturity, seed output |
| `fecundity` | 0..1 | More seeds; lowers seed mass or seedling survival |
| `seed_mass` | 0..1 | Better establishment; fewer seeds and shorter dispersal |
| `dispersal` | 0..1 | Farther spread; lower establishment |
| `timing` | 0..1 | Faster maturity; shorter lifespan or smaller adult size |

These genes are still readable enough to tune, but no longer behave like simple
appearance sliders.

## Phenotype Mapping Sketch

Compute local environment:

```text
moisture = terrain/rivers/biome moisture at plant
altitude = normalized height
shade = nearby canopy pressure
root_competition = nearby below-ground pressure
```

Compute match:

```text
wet_match = bell(moisture, wet_pref, width = stress_width)
alt_match = bell(altitude, alt_pref, width = stress_width)

specialist_bonus = lerp(1.15, 0.85, stress_width)
abiotic_fitness = wet_match * alt_match * specialist_bonus
```

Compute trade-offs:

```text
water_need = 0.4 + 0.8*capture
maturity_scale = 0.75 + 0.65*capture - 0.25*timing
lifespan_scale = 0.7 + 0.6*(1.0 - timing)

seed_count = base_seed_count
  * lerp(0.6, 1.4, fecundity)
  * lerp(1.25, 0.55, seed_mass)
  * lerp(1.0, 0.75, capture)

establishment = abiotic_fitness
  * lerp(0.6, 1.4, seed_mass)
  * competition_room
  * dispersal_establishment_penalty
```

Compute visible traits:

```text
realized_height = base_height * growth_stage_scale * capture * abiotic_fitness * water_affordance
realized_width = base_width * (0.7 + 0.6*capture) * abiotic_fitness
leaf_color = species_leaf shifted by stress, moisture, and perhaps hidden gene effects
stress_visual = 1.0 - abiotic_fitness * competition_room
```

The exact numbers are tuning placeholders. The important structure is that
phenotype comes from many interacting causes.

## Population Divergence And New Niches

The system should make several kinds of divergence possible:

- **Clines** - gradual shifts along moisture or altitude gradients.
- **Local adaptation** - a valley population becomes better at valley
  conditions than its ancestors.
- **Generalist/specialist balance** - broad-tolerance plants persist across many
  sites, but specialists dominate ideal patches.
- **Character displacement** - competing lineages diverge because similar plants
  suppress each other more strongly.
- **Founder effects** - a few seeds colonize an isolated patch and drift before
  selection refines them.
- **Hybrid zones** - if recombination is added, neighbouring populations can
  mix at habitat boundaries.

None of these should require authoring "the wetland variant" or "the ridge
variant". The player should discover them.

## Making Evolution Visible

Evolution must be observable or it will feel like hidden bookkeeping.

Recommended tools:

- **Evolution lens** - toggle plant coloring by wet preference, altitude
  preference, abiotic fitness, competition stress, lineage, or generation.
- **Region inspector** - mean genes, phenotype averages, diversity, birth rate,
  death rate, seed establishment rate, dominant lineage.
- **Plant inspector** - selected plant's genotype, phenotype, local fitness,
  parents, generation, and recent mutations.
- **Time-lapse mode** - accelerate days and snapshot a region's population
  colors over time.
- **Niche labels** - debug summaries such as "dry ridge specialists", "wet
  lowland generalists", or "fast colonizers", inferred from phenotype clusters.
- **Mutation highlights** - optional debug markers for rare successful mutants
  whose descendants are spreading.

Normal rendering should remain natural. Debug overlays reveal the hidden
biology when the player asks for it.

## Persistence And Determinism

Base flora can receive deterministic founder genotypes from a hash of:

```text
world_seed + canonical_chunk + plant_index + species
```

Those founders can be phenotype-mapped like any other plant, but they should not
need per-plant genotype storage unless they enter the spreading population as
parents.

Spreading plants need persisted genetic state. The current packed `Plant` record
is only 16 bytes, so this will require a storage-format decision:

- Add compact gene bytes directly to spread plants.
- Store genotypes in a parallel spread-genetics blob.
- Store a lineage id per plant and keep lineage genotypes separately, if many
  siblings share genes.

The simplest robust first version is probably one byte per gene for spread
plants, with a version bump for `plants.bin`. Base-snapshot format can remain
separate unless founder genotypes must be inspectable without recomputing them.

## First Version Scope

First playable version:

- Eight named ecological genes.
- Genotype -> phenotype function with explicit trade-offs.
- Fitness from moisture, altitude, and local competition.
- Inheritance by parent copy plus mutation.
- Selection through seed count, seed landing, establishment, growth, stress
  color, lifespan, and spread chance.
- Hashed founder genotypes for base flora.
- Persisted genotypes for spreading plants.
- Region readout and at least one evolution overlay.

Strong next additions:

- Recombination with a nearby mature same-species parent.
- Niche clustering/readable emergent labels.
- More visible morphology: quantized crown/branch/prototype variants selected
  by phenotype bands, not unique meshes per plant.

## Design Rules To Keep

1. **Genotype is not phenotype.** Genes are inherited causes; traits are
   realized outcomes.
2. **No gene is purely good.** Every advantage has a cost.
3. **Selection happens through life events.** Survival, maturity, seed output,
   seed dispersal, establishment, and competition carry the evolutionary force.
4. **Environment is half the organism.** The same genotype can look and perform
   differently in different places.
5. **Competition creates niches.** Terrain gradients make clines; neighbours
   make ecological drama.
6. **Few genes, rich interactions.** Add mapping depth before adding gene count.
7. **Visible beats invisible.** Debug lenses and readable visual stress are part
   of the feature, not polish.
8. **Static world stays static.** Evolution lives primarily in the spreading
   population so generation and saves stay tractable.
