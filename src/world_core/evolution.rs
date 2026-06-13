use glam::IVec2;
use serde::{Deserialize, Serialize};

use crate::world_core::content::sampling::{hash4, hash_to_unit_float};
use crate::world_core::herbarium::PlantSpeciesInfo;

pub const EVOLUTION_ALTITUDE_MAX: f32 = 220.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionOverlayMode {
    #[default]
    Off,
    WetPreference,
    AltitudePreference,
    AbioticFitness,
    CompetitionStress,
    Generation,
}

impl EvolutionOverlayMode {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::WetPreference,
        Self::AltitudePreference,
        Self::AbioticFitness,
        Self::CompetitionStress,
        Self::Generation,
    ];

    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

impl PlantGenes {
    pub const BYTE_LEN: usize = 8;

    pub const fn from_bytes(bytes: [u8; Self::BYTE_LEN]) -> Self {
        Self {
            wet_pref: bytes[0],
            alt_pref: bytes[1],
            stress_width: bytes[2],
            capture: bytes[3],
            fecundity: bytes[4],
            seed_mass: bytes[5],
            dispersal: bytes[6],
            timing: bytes[7],
        }
    }

    pub const fn to_bytes(self) -> [u8; Self::BYTE_LEN] {
        [
            self.wet_pref,
            self.alt_pref,
            self.stress_width,
            self.capture,
            self.fecundity,
            self.seed_mass,
            self.dispersal,
            self.timing,
        ]
    }

    pub fn unit(field: u8) -> f32 {
        field as f32 / u8::MAX as f32
    }
}

impl Default for PlantGenes {
    fn default() -> Self {
        Self::from_bytes([128; Self::BYTE_LEN])
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlantEnvironment {
    pub moisture: f32,
    pub altitude: f32,
    pub river_wetness: f32,
    pub shade: f32,
    pub root_pressure: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FounderKey {
    pub chunk: IVec2,
    pub local_x: u16,
    pub local_z: u16,
    pub species: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MutationKey {
    pub chunk: IVec2,
    pub parent_index: u32,
    pub bucket: u32,
    pub seed_index: u32,
}

pub fn normalize_altitude(height: f32, sea_level: f32) -> f32 {
    ((height - sea_level) / (EVOLUTION_ALTITUDE_MAX - sea_level).max(1.0)).clamp(0.0, 1.0)
}

pub fn initial_genes(seed: u32, key: FounderKey, env: PlantEnvironment) -> PlantGenes {
    let wet = (env.moisture + signed_jitter(seed, key, 11) * 0.25).clamp(0.0, 1.0);
    let alt = (env.altitude + signed_jitter(seed, key, 12) * 0.25).clamp(0.0, 1.0);
    PlantGenes {
        wet_pref: byte(wet),
        alt_pref: byte(alt),
        stress_width: moderate_gene(seed, key, 13),
        capture: moderate_gene(seed, key, 14),
        fecundity: moderate_gene(seed, key, 15),
        seed_mass: moderate_gene(seed, key, 16),
        dispersal: moderate_gene(seed, key, 17),
        timing: moderate_gene(seed, key, 18),
    }
}

pub fn inherit_and_mutate(parent: PlantGenes, seed: u32, key: MutationKey) -> PlantGenes {
    const MUTATION_CHANCE: f32 = 0.08;
    const MAX_STEP: i16 = 10;

    let mut bytes = parent.to_bytes();
    for (idx, byte) in bytes.iter_mut().enumerate() {
        let roll = mutation_hash(seed, key, idx as u32 * 2);
        if roll >= MUTATION_CHANCE {
            continue;
        }
        let step_unit = mutation_hash(seed, key, idx as u32 * 2 + 1);
        let magnitude = 1 + (step_unit * MAX_STEP as f32).floor() as i16;
        let sign = if step_unit < 0.5 { -1 } else { 1 };
        *byte = (*byte as i16 + sign * magnitude).clamp(0, u8::MAX as i16) as u8;
    }
    PlantGenes::from_bytes(bytes)
}

pub fn evaluate_phenotype(
    genes: PlantGenes,
    species: &PlantSpeciesInfo,
    env: PlantEnvironment,
) -> PlantPhenotype {
    let wet_pref = PlantGenes::unit(genes.wet_pref);
    let alt_pref = PlantGenes::unit(genes.alt_pref);
    let stress_width = PlantGenes::unit(genes.stress_width);
    let capture = PlantGenes::unit(genes.capture);
    let fecundity = PlantGenes::unit(genes.fecundity);
    let seed_mass = PlantGenes::unit(genes.seed_mass);
    let dispersal = PlantGenes::unit(genes.dispersal);
    let timing = PlantGenes::unit(genes.timing);

    let tolerance = 0.12 + stress_width * 0.48;
    let moisture = (env.moisture + env.river_wetness * 0.15).clamp(0.0, 1.0);
    let wet_fit = niche_fit(moisture, wet_pref, tolerance);
    let alt_fit = niche_fit(env.altitude.clamp(0.0, 1.0), alt_pref, tolerance);
    let abiotic_fitness = (wet_fit * alt_fit).sqrt().clamp(0.0, 1.0);

    let competition_pressure =
        (env.shade * (0.35 + capture * 0.65) + env.root_pressure).clamp(0.0, 1.0);
    let competition_room = (1.0 - competition_pressure).clamp(0.0, 1.0);
    let stress = (1.0 - abiotic_fitness * competition_room).clamp(0.0, 1.0);

    let capture_height_bonus = 0.75 + capture * 0.65;
    let stress_cost = 1.0 - stress * 0.35;
    let height_scale = (capture_height_bonus * stress_cost).clamp(0.45, 1.45);
    let width_scale = (0.8 + capture * 0.45 - dispersal * 0.15).clamp(0.5, 1.35);

    let maturity_scale = (1.25 - timing * 0.45 + seed_mass * 0.2 + stress * 0.35).clamp(0.55, 1.8);
    let lifespan_scale =
        (0.75 + timing * 0.55 + abiotic_fitness * 0.35 - fecundity * 0.25).clamp(0.45, 1.8);
    let seed_count_scale = (0.45 + fecundity * 1.35
        - seed_mass * 0.55
        - capture * 0.15
        - stress_width * 0.18
        - stress * 0.65)
        .clamp(0.05, 1.9);
    let spread_radius_scale = (0.45 + dispersal * 1.35 - seed_mass * 0.45).clamp(0.1, 1.85);
    let establishment_chance =
        (0.08 + abiotic_fitness * 0.58 + seed_mass * 0.28 + stress_width * 0.12
            - dispersal * 0.18
            - fecundity * 0.08)
            .clamp(0.01, 0.98)
            * competition_room;

    let tint_stress = stress * 0.45;
    let leaf_tint = [
        (1.0 + tint_stress * 0.75).clamp(0.0, 1.35),
        (1.0 - tint_stress * 0.35).clamp(0.0, 1.2),
        (1.0 - tint_stress * 0.55).clamp(0.0, 1.2),
        if species.placement.aquatic { 0.92 } else { 1.0 },
    ];

    PlantPhenotype {
        abiotic_fitness,
        competition_room,
        stress,
        height_scale,
        width_scale,
        maturity_scale,
        lifespan_scale,
        seed_count_scale,
        spread_radius_scale,
        establishment_chance,
        leaf_tint,
    }
}

fn niche_fit(actual: f32, preferred: f32, tolerance: f32) -> f32 {
    let distance = (actual - preferred).abs();
    (1.0 - distance / tolerance.max(0.001)).clamp(0.0, 1.0)
}

fn moderate_gene(seed: u32, key: FounderKey, salt: u32) -> u8 {
    byte(0.35 + hash_unit(seed, key, salt) * 0.4)
}

fn signed_jitter(seed: u32, key: FounderKey, salt: u32) -> f32 {
    hash_unit(seed, key, salt) * 2.0 - 1.0
}

fn hash_unit(seed: u32, key: FounderKey, salt: u32) -> f32 {
    let local = ((key.local_x as u32) << 16) | key.local_z as u32;
    let chunk = ((key.chunk.x as u32) << 16) ^ key.chunk.y as u32;
    hash_to_unit_float(hash4(
        seed.wrapping_add(salt),
        chunk,
        local,
        key.species as u32,
    ))
}

fn mutation_hash(seed: u32, key: MutationKey, salt: u32) -> f32 {
    hash_to_unit_float(hash4(
        seed.wrapping_add(0xA700).wrapping_add(salt),
        ((key.chunk.x as u32) << 16) ^ key.chunk.y as u32,
        key.parent_index ^ key.bucket.wrapping_mul(0x9E37_79B9),
        key.seed_index,
    ))
}

fn byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * u8::MAX as f32).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_core::herbarium::{Herbarium, PlantRegistry};

    fn species() -> PlantSpeciesInfo {
        PlantRegistry::from_herbarium(&Herbarium::default_seeded()).species[0].clone()
    }

    fn env(moisture: f32, altitude: f32) -> PlantEnvironment {
        PlantEnvironment {
            moisture,
            altitude,
            river_wetness: 0.0,
            shade: 0.0,
            root_pressure: 0.0,
        }
    }

    fn genes(bytes: [u8; PlantGenes::BYTE_LEN]) -> PlantGenes {
        PlantGenes::from_bytes(bytes)
    }

    #[test]
    fn genes_round_trip_stable_byte_layout() {
        let bytes = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(PlantGenes::from_bytes(bytes).to_bytes(), bytes);
    }

    #[test]
    fn specialist_beats_generalist_at_ideal_site_and_loses_off_niche() {
        let species = species();
        let specialist = genes([204, 76, 35, 128, 128, 128, 128, 128]);
        let generalist = genes([128, 128, 230, 128, 128, 128, 128, 128]);

        let ideal_specialist = evaluate_phenotype(specialist, &species, env(0.8, 0.3));
        let ideal_generalist = evaluate_phenotype(generalist, &species, env(0.8, 0.3));
        assert!(ideal_specialist.abiotic_fitness > ideal_generalist.abiotic_fitness);

        let off_specialist = evaluate_phenotype(specialist, &species, env(0.2, 0.85));
        let off_generalist = evaluate_phenotype(generalist, &species, env(0.2, 0.85));
        assert!(off_generalist.abiotic_fitness > off_specialist.abiotic_fitness);
    }

    #[test]
    fn seed_mass_and_dispersal_have_tradeoffs() {
        let species = species();
        let base = genes([128, 128, 180, 128, 128, 128, 128, 128]);
        let heavy_seed = PlantGenes {
            seed_mass: 240,
            ..base
        };
        let light_seed = PlantGenes {
            seed_mass: 20,
            ..base
        };
        let far_seed = PlantGenes {
            dispersal: 240,
            ..base
        };
        let near_seed = PlantGenes {
            dispersal: 20,
            ..base
        };

        let heavy = evaluate_phenotype(heavy_seed, &species, env(0.5, 0.5));
        let light = evaluate_phenotype(light_seed, &species, env(0.5, 0.5));
        assert!(heavy.establishment_chance > light.establishment_chance);
        assert!(heavy.seed_count_scale < light.seed_count_scale);

        let far = evaluate_phenotype(far_seed, &species, env(0.5, 0.5));
        let near = evaluate_phenotype(near_seed, &species, env(0.5, 0.5));
        assert!(far.spread_radius_scale > near.spread_radius_scale);
        assert!(far.establishment_chance < near.establishment_chance);
    }

    #[test]
    fn every_gene_has_a_measurable_upside_and_downside() {
        let species = species();
        let base = genes([128, 128, 128, 128, 128, 128, 128, 128]);

        let wet_low = PlantGenes {
            wet_pref: 25,
            ..base
        };
        let wet_high = PlantGenes {
            wet_pref: 230,
            ..base
        };
        assert!(
            evaluate_phenotype(wet_high, &species, env(0.9, 0.5)).abiotic_fitness
                > evaluate_phenotype(wet_low, &species, env(0.9, 0.5)).abiotic_fitness
        );
        assert!(
            evaluate_phenotype(wet_low, &species, env(0.1, 0.5)).abiotic_fitness
                > evaluate_phenotype(wet_high, &species, env(0.1, 0.5)).abiotic_fitness
        );

        let alt_low = PlantGenes {
            alt_pref: 25,
            ..base
        };
        let alt_high = PlantGenes {
            alt_pref: 230,
            ..base
        };
        assert!(
            evaluate_phenotype(alt_high, &species, env(0.5, 0.9)).abiotic_fitness
                > evaluate_phenotype(alt_low, &species, env(0.5, 0.9)).abiotic_fitness
        );
        assert!(
            evaluate_phenotype(alt_low, &species, env(0.5, 0.1)).abiotic_fitness
                > evaluate_phenotype(alt_high, &species, env(0.5, 0.1)).abiotic_fitness
        );

        let narrow = PlantGenes {
            stress_width: 25,
            ..base
        };
        let broad = PlantGenes {
            stress_width: 230,
            ..base
        };
        assert!(
            evaluate_phenotype(broad, &species, env(0.75, 0.75)).abiotic_fitness
                > evaluate_phenotype(narrow, &species, env(0.75, 0.75)).abiotic_fitness
        );
        assert!(
            evaluate_phenotype(narrow, &species, env(0.5, 0.5)).seed_count_scale
                > evaluate_phenotype(broad, &species, env(0.5, 0.5)).seed_count_scale
        );

        let low_capture = PlantGenes {
            capture: 25,
            ..base
        };
        let high_capture = PlantGenes {
            capture: 230,
            ..base
        };
        assert!(
            evaluate_phenotype(high_capture, &species, env(0.5, 0.5)).height_scale
                > evaluate_phenotype(low_capture, &species, env(0.5, 0.5)).height_scale
        );
        assert!(
            evaluate_phenotype(low_capture, &species, env(0.5, 0.5)).seed_count_scale
                > evaluate_phenotype(high_capture, &species, env(0.5, 0.5)).seed_count_scale
        );

        let low_fecundity = PlantGenes {
            fecundity: 25,
            ..base
        };
        let high_fecundity = PlantGenes {
            fecundity: 230,
            ..base
        };
        assert!(
            evaluate_phenotype(high_fecundity, &species, env(0.5, 0.5)).seed_count_scale
                > evaluate_phenotype(low_fecundity, &species, env(0.5, 0.5)).seed_count_scale
        );
        assert!(
            evaluate_phenotype(low_fecundity, &species, env(0.5, 0.5)).lifespan_scale
                > evaluate_phenotype(high_fecundity, &species, env(0.5, 0.5)).lifespan_scale
        );

        let low_timing = PlantGenes { timing: 25, ..base };
        let high_timing = PlantGenes {
            timing: 230,
            ..base
        };
        assert!(
            evaluate_phenotype(low_timing, &species, env(0.5, 0.5)).maturity_scale
                > evaluate_phenotype(high_timing, &species, env(0.5, 0.5)).maturity_scale
        );
        assert!(
            evaluate_phenotype(high_timing, &species, env(0.5, 0.5)).lifespan_scale
                > evaluate_phenotype(low_timing, &species, env(0.5, 0.5)).lifespan_scale
        );
    }

    #[test]
    fn extreme_gene_sets_stay_bounded() {
        let species = species();
        for genes in [
            PlantGenes::from_bytes([0; PlantGenes::BYTE_LEN]),
            PlantGenes::from_bytes([255; PlantGenes::BYTE_LEN]),
            PlantGenes::from_bytes([0, 255, 0, 255, 0, 255, 0, 255]),
        ] {
            let p = evaluate_phenotype(
                genes,
                &species,
                PlantEnvironment {
                    moisture: 1.5,
                    altitude: -0.5,
                    river_wetness: 0.7,
                    shade: 1.2,
                    root_pressure: 1.3,
                },
            );
            assert!((0.0..=1.0).contains(&p.abiotic_fitness));
            assert!((0.0..=1.0).contains(&p.competition_room));
            assert!((0.0..=1.0).contains(&p.stress));
            assert!((0.45..=1.45).contains(&p.height_scale));
            assert!((0.5..=1.35).contains(&p.width_scale));
            assert!((0.55..=1.8).contains(&p.maturity_scale));
            assert!((0.45..=1.8).contains(&p.lifespan_scale));
            assert!((0.05..=1.9).contains(&p.seed_count_scale));
            assert!((0.1..=1.85).contains(&p.spread_radius_scale));
            assert!((0.0..=0.98).contains(&p.establishment_chance));
            assert!(p.leaf_tint.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn mutation_is_deterministic_clamped_and_near_parent() {
        let parent = PlantGenes::from_bytes([0, 4, 64, 120, 128, 192, 250, 255]);
        let key = MutationKey {
            chunk: IVec2::new(3, 9),
            parent_index: 12,
            bucket: 34,
            seed_index: 1,
        };
        let a = inherit_and_mutate(parent, 7, key);
        let b = inherit_and_mutate(parent, 7, key);
        assert_eq!(a, b);

        for (child, parent) in a.to_bytes().into_iter().zip(parent.to_bytes()) {
            let delta = child.abs_diff(parent);
            assert!(delta == 0 || delta <= 10);
        }
    }
}
