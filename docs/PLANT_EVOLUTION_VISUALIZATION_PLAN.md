# Plant Evolution Visualization Plan

> Status: implementation planning. This document describes how to expose,
> aggregate, and visualize the plant evolution simulation added in PR 141.
> The plan is intentionally phased: first make the right data available, then
> layer richer in-game charts, map views, and population analysis on top.

## Goal

Give the player a readable way to ask:

- What genes are common in this plant population?
- What phenotypes are those genes producing here?
- How are genes and phenotypes changing over simulation time?
- Are nearby areas diverging into different local populations?
- How do geography, environment, competition, and generation age explain the
  differences we see?

The first version should be useful even before advanced charts exist. A small
dialogue with trustworthy region statistics is more valuable than a beautiful
panel fed by incomplete data.

## Design Principles

- **Data before decoration.** Build stable query/aggregation APIs first, then
  add chart types as consumers.
- **Genotype and phenotype stay separate.** Genes are inherited bytes.
  Phenotypes are derived from genes, species baseline, environment, and
  competition.
- **Every visualization should have a scope.** Global, visible chunks, selected
  region, selected species, and selected plant are different questions.
- **All flora is one ecology.** Reports should not distinguish base flora from
  plants born during play. The simulation may store or generate them
  differently, but the player-facing analysis should treat them as one
  population.
- **Charts should explain, not just display.** Prefer comparisons, trends, and
  outliers over dumping every value.
- **Debug API parity matters.** Any in-game visualization should be backed by
  data that can also be inspected through the debug API or CLI.

## Product Decisions

- The default local sampling scope is chunk-sized: use one
  `CHUNK_SIZE_METERS` radius, currently 256 m, unless a later UI switches to an
  exact current-chunk cell selection.
- Base flora and born-during-play flora should be indistinguishable in reports,
  charts, maps, and population summaries.
- Global summaries and histories should include every plant unless a later
  explicit filter is added for debugging.
- The live Population Lens follows the camera by default. Time-series history
  is pinned to fixed world regions, so moving the camera does not change what a
  chart means.
- Asking for history from the live lens should create or select a pinned watcher
  at the lens's current center/radius.
- First-version time-series history is session-only. It resets when the game
  restarts; persistence can be added later once the charts prove useful.

## Existing Foundation

PR 141 already gives us the first data surface:

- `PlantGenes` has eight byte-valued genes:
  - `wet_pref`
  - `alt_pref`
  - `stress_width`
  - `capture`
  - `fecundity`
  - `seed_mass`
  - `dispersal`
  - `timing`
- `evaluate_phenotype()` derives phenotype values from genes and local
  environment.
- `PlantWorld::inspect_evolution_region()` returns region-level JSON with:
  - gene mean/stddev
  - phenotype means
  - plant count
  - stage counts
  - top species
  - mean abiotic fitness
  - mean competition stress
- Evolution overlay modes already exist for:
  - wet preference
  - altitude preference
  - abiotic fitness
  - competition stress
  - generation

That means the first visualization can be implemented as a reader of existing
simulation state, not as a new simulation subsystem.

## Phase 1: Stable Evolution Data Contract

### Purpose

Turn the existing region inspector into a typed, reusable data contract that
can feed egui panels, the debug API, the debug CLI, tests, and future charts.

### Work

- Replace or wrap the current ad-hoc `serde_json::Value` region response with
  serializable Rust structs.
- Keep JSON compatibility for the debug API.
- Add explicit query scope fields:
  - center `x/z`
  - radius
  - world-wrapped center
  - sample time in simulation hours
  - optional species filter
- Add sample quality fields:
  - total plants considered
  - plants included
  - empty-region flag
  - number of chunks touched
- Add per-gene aggregate fields:
  - mean
  - stddev
  - min/max
  - maybe p10/p50/p90 if cheap enough
- Add per-phenotype aggregate fields:
  - mean
  - stddev where useful
- Keep stage and species counts.

### Output Data

```text
EvolutionRegionReport
  scope
  counts
  genes[8]
  phenotypes
  stages
  species
  environment
```

### Acceptance Criteria

- `inspect_evolution_region` still works from the debug CLI.
- Existing tests for region inspection are updated to assert typed fields.
- Empty regions return a valid report instead of special-case UI behavior.
- The report is cheap enough to run interactively for a local radius.

## Phase 2: Basic In-Game Population Dialogue

### Purpose

Make the simulation legible in-game with a minimal dialogue that reads the
Phase 1 report.

### Work

- Add an egui dialogue, tentatively named **Population Lens**.
- Open it from a keybind or existing debug/config UI.
- Query a chunk-sized region around the camera position by default.
- Let the player adjust radius.
- Show:
  - plant count
  - top species
  - lifecycle stage distribution
  - mean fitness and stress
  - gene bars with variance bands
  - phenotype summary bars
- Add buttons or segmented controls for the existing evolution overlay modes.

### First UI Layout

```text
Population Lens

[radius control] [overlay mode]

Population
  plants, species, stages

Genes
  wet_pref        mean + variance band
  alt_pref        mean + variance band
  ...

Phenotype Here
  fitness, stress, height, width, seeds, spread, establishment
```

### Acceptance Criteria

- The dialogue is useful while flying around without pausing the simulation.
- The UI handles empty and sparse regions cleanly.
- Values match the debug CLI report for the same center/radius.
- The live report follows the camera unless the player explicitly pins or
  selects a fixed region.
- No history or map aggregation is required yet.

## Phase 3: Watched Regions And Time Series

### Purpose

Capture how genes and phenotypes change over simulation time.

### Work

- Add a lightweight history recorder in the runtime.
- Record snapshots at coarse simulation intervals, not every frame.
- Support a few watched scopes:
  - global
  - pinned world positions created from the current Population Lens region
  - pinned world positions created from map selection
  - optional species-specific filters
- Do not record a camera-following history series. A time series should describe
  one fixed place over time.
- Store bounded ring buffers in memory.
- Do not persist history in the first version.
- Add debug API commands for:
  - list watched regions
  - add/remove watched region
  - fetch history

### Output Data

```text
EvolutionHistorySeries
  scope
  interval_hours
  samples[]
    sim_hour
    plant_count
    gene_means[8]
    gene_stddevs[8]
    phenotype_means
    species_counts
    stage_counts
```

### First Charts

- Small sparklines for selected genes.
- Fitness/stress trend line.
- Plant count trend line.
- Diversity trend line using mean gene stddev.

### Acceptance Criteria

- Time series continue updating while the dialogue is open.
- History memory use is bounded and predictable.
- The first chart can answer: "Is this population changing?"

## Phase 4: Spatial Aggregation Grid

### Purpose

Make geography visible without scanning every plant every frame.

### Work

- Build a coarse world grid aggregator for evolution data.
- Each grid cell summarizes plants in its area.
- Cache summaries and refresh after growth/spread ticks or on demand.
- Include enough data to drive both map overlays and dialogue drill-downs.
- Keep resolution configurable; start coarse.

### Output Data

```text
EvolutionMapGrid
  resolution
  cells[]
    plant_count
    species_counts
    gene_means[8]
    gene_stddevs[8]
    phenotype_means
    mean_environment
    mean_generation_age
```

### Visualizations

- Heatmap by selected gene mean.
- Heatmap by gene diversity.
- Heatmap by fitness or stress.
- Density map.
- Dominant species map.
- Existing plant tint overlays remain useful for close-up inspection.

### Acceptance Criteria

- The map can show broad regional differences instantly.
- Sparse cells are visually distinct from low-value cells.
- The dialogue can select a map cell and show its report.

## Phase 5: Population Structure Detection

### Purpose

Answer whether distinct populations exist, rather than only showing raw gene
gradients.

### Work

- Start with simple, explainable clustering over spatial grid cells.
- Cluster by:
  - species
  - gene means
  - phenotype means
  - geographic adjacency
- Prefer conservative labels over pretending we found formal species.
- Track cluster stability over time once history exists.

### Output Data

```text
EvolutionPopulationCluster
  id
  species
  cell_count
  plant_count
  centroid
  gene_profile
  phenotype_profile
  distinguishing_traits
  stability_score
```

### Visualizations

- Population regions on the world map.
- Cluster list in the dialogue.
- Comparison between selected cluster and local/global baseline.
- Short interpretation text:
  - "Wetter than global oak average."
  - "Higher dispersal but lower seed mass."
  - "High stress, low establishment."

### Acceptance Criteria

- Clusters are optional overlays, not required to understand basic data.
- The player can compare at least two detected populations.
- Cluster labels are generated from measurable differences.

## Phase 6: Phenotype Preview And Plant-Level Inspection

### Purpose

Connect abstract statistics back to visible plant form.

### Work

- Add selected plant inspection:
  - species
  - genes
  - derived phenotype at current site
  - age/stage
  - local environment
  - local competition
- Add population-average phenotype comparison:
  - selected region vs global species average
  - selected cluster vs neighboring cluster
- Consider a small procedural preview later:
  - baseline species silhouette
  - average local phenotype silhouette
  - selected plant silhouette

### Acceptance Criteria

- A player can look at a plant and understand why it appears stressed, large,
  small, fast-growing, or locally successful.
- Phenotype values are described as "expressed here", not as permanent stored
  facts.

## Phase 7: Advanced Analysis

These are deliberately later features.

- Lineage tracking and ancestry trees.
- Recombination/parent contribution visualization.
- Mutation event markers.
- Save/load of long-term history.
- Selection-pressure decomposition:
  - moisture mismatch
  - altitude mismatch
  - shade
  - root competition
  - dispersal cost
- Export reports for offline analysis.
- Comparison between separate game instances or seeds.

## Suggested Build Order

1. Type the existing region report.
2. Add missing aggregate fields: min/max and environment means.
3. Build the basic Population Lens dialogue.
4. Add in-memory watched-region history.
5. Add sparklines for watched-region genes.
6. Add coarse spatial aggregation grid.
7. Add map heatmaps from the grid.
8. Add simple population clustering.
9. Add plant-level inspection and phenotype comparison.

## First Milestone

The first milestone should be:

- a typed `EvolutionRegionReport`
- a debug CLI response using that report
- an in-game Population Lens dialogue around the camera
- no history, no clustering, no advanced charts

That milestone is enough to validate the data model and make the PR 141
simulation visible to the player. Everything after that can be built as a more
specialized view over the same reports and aggregations.
