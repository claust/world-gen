//! Player-facing gameplay scoring derived from the worldwide census.
//!
//! The simulation already tallies a per-species × per-stage census
//! ([`PopulationSample`]); the Agency MVP turns that raw tally into two headline
//! numbers the player can watch respond to their planting and culling:
//!
//! - **Biomass** — living plants weighted by growth stage, so a mature forest
//!   scores far above a field of seedlings. Dead snags contribute nothing.
//! - **Biodiversity** — a Shannon index over the living per-species totals, so a
//!   balanced mix of species scores above a monoculture of the same size.
//!
//! Both are pure functions of a single [`PopulationSample`], so the panel and the
//! debug API read identical values.

use crate::world_core::lifecycle::GrowthStage;
use crate::world_core::population_history::{stage_index, PopulationSample, STAGE_COUNT};

/// Biomass contribution of one plant at each growth stage, indexed in canonical
/// stage order. Mirrors [`GrowthStage::scale_factor`] for the living stages so a
/// plant's biomass tracks its rendered size; dead snags count as zero living
/// biomass (they are decaying, not growing).
fn stage_biomass_weights() -> [f64; STAGE_COUNT] {
    let mut w = [0.0; STAGE_COUNT];
    w[stage_index(GrowthStage::Seedling)] = GrowthStage::Seedling.scale_factor() as f64;
    w[stage_index(GrowthStage::Young)] = GrowthStage::Young.scale_factor() as f64;
    w[stage_index(GrowthStage::Mature)] = GrowthStage::Mature.scale_factor() as f64;
    w[stage_index(GrowthStage::Dead)] = 0.0;
    w
}

/// Whether a stage counts as a living plant for population/biodiversity tallies.
fn is_living(index: usize) -> bool {
    index != stage_index(GrowthStage::Dead)
}

/// Headline ecosystem stats for the gameplay HUD, computed from one census.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EcoScore {
    /// Living plants (seedling + young + mature) across the whole world.
    pub living: u64,
    /// Stage-weighted living biomass. Unitless; a mature plant contributes 1.0.
    pub biomass: f64,
    /// Number of species with at least one living individual.
    pub species_present: usize,
    /// Shannon diversity index (natural log) over living per-species totals. `0.0`
    /// for an empty world or a single-species monoculture.
    pub shannon: f64,
}

impl EcoScore {
    /// Reduce a worldwide census to the headline biomass/biodiversity numbers.
    pub fn from_sample(sample: &PopulationSample) -> Self {
        let weights = stage_biomass_weights();
        let mut living = 0u64;
        let mut biomass = 0.0;
        // Upper bound is one entry per species; pre-size to avoid reallocation
        // as living species are pushed (this runs on the HUD/debug path).
        let mut living_per_species: Vec<f64> = Vec::with_capacity(sample.per_species.len());

        for row in &sample.per_species {
            let mut species_living = 0u64;
            for (i, &count) in row.iter().enumerate() {
                biomass += count as f64 * weights[i];
                if is_living(i) {
                    species_living += count as u64;
                }
            }
            living += species_living;
            if species_living > 0 {
                living_per_species.push(species_living as f64);
            }
        }

        let total: f64 = living_per_species.iter().sum();
        let shannon = if total > 0.0 {
            -living_per_species
                .iter()
                .map(|&c| {
                    let p = c / total;
                    p * p.ln()
                })
                .sum::<f64>()
        } else {
            0.0
        };

        Self {
            living,
            biomass,
            species_present: living_per_species.len(),
            shannon,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(rows: Vec<[u32; STAGE_COUNT]>) -> PopulationSample {
        PopulationSample {
            hour: 0.0,
            per_species: rows,
        }
    }

    #[test]
    fn empty_world_scores_zero() {
        let s = EcoScore::from_sample(&sample(vec![[0, 0, 0, 0], [0, 0, 0, 0]]));
        assert_eq!(s.living, 0);
        assert_eq!(s.biomass, 0.0);
        assert_eq!(s.species_present, 0);
        assert_eq!(s.shannon, 0.0);
    }

    #[test]
    fn dead_snags_are_not_living_biomass() {
        // 10 dead of one species, nothing alive.
        let s = EcoScore::from_sample(&sample(vec![[0, 0, 0, 10]]));
        assert_eq!(s.living, 0);
        assert_eq!(s.biomass, 0.0);
        assert_eq!(s.species_present, 0);
    }

    #[test]
    fn biomass_weights_by_stage() {
        // 1 seedling + 1 young + 1 mature of one species.
        let s = EcoScore::from_sample(&sample(vec![[1, 1, 1, 0]]));
        assert_eq!(s.living, 3);
        let expected = GrowthStage::Seedling.scale_factor() as f64
            + GrowthStage::Young.scale_factor() as f64
            + GrowthStage::Mature.scale_factor() as f64;
        assert!((s.biomass - expected).abs() < 1e-9);
        assert_eq!(s.species_present, 1);
        assert_eq!(s.shannon, 0.0); // one species → no diversity
    }

    #[test]
    fn even_split_maximizes_shannon() {
        // Two species, equal living counts → Shannon = ln(2).
        let even = EcoScore::from_sample(&sample(vec![[0, 0, 5, 0], [0, 0, 5, 0]]));
        assert!((even.shannon - 2.0f64.ln()).abs() < 1e-9);
        assert_eq!(even.species_present, 2);

        // Skewed split of the same total scores lower.
        let skewed = EcoScore::from_sample(&sample(vec![[0, 0, 9, 0], [0, 0, 1, 0]]));
        assert!(skewed.shannon < even.shannon);
    }
}
