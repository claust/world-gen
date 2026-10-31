//! Player-founded strains — named lineages the breeder loop tracks.
//!
//! A strain is founded on one plant (or born from a hand cross-pollination) and
//! inherited down the maternal line by every seed the spread pass emits, so it
//! names a real lineage rather than a label the player paints on. The plant
//! store owns the registry and persistence; this module holds the pure data:
//! the registry records, generated names, and the per-strain report the HUD and
//! the trait expeditions read.

use crate::world_core::evolution::PlantGenes;
use crate::world_core::population_history::STAGE_COUNT;

/// Where a strain's founder grew, so the tag can be re-applied to a base-flora
/// founder on load (base plants are regenerated from the seed, not saved).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StrainFounder {
    pub chunk: u32,
    pub local_x: u16,
    pub local_z: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Strain {
    /// Non-zero; `Plant::strain` holds it. Ids are `index + 1` in the registry.
    pub id: u16,
    pub name: String,
    pub species: u8,
    /// Born from a player cross-pollination rather than tagged on a wild plant.
    pub hybrid: bool,
    pub founded_hour: f64,
    pub founder: StrainFounder,
}

/// Gene loci in [`PlantGenes::to_bytes`] order, for reports and expeditions.
pub const GENE_NAMES: [&str; PlantGenes::BYTE_LEN] = [
    "wet_pref",
    "alt_pref",
    "stress_width",
    "capture",
    "fecundity",
    "seed_mass",
    "dispersal",
    "timing",
];

pub const DISPERSAL: usize = 6;

/// Live summary of one strain, from a scan of the chunks it has reached.
#[derive(Clone, Debug, PartialEq)]
pub struct StrainReport {
    pub id: u16,
    pub name: String,
    pub species: u8,
    pub hybrid: bool,
    /// Seedling + young + mature members.
    pub living: u32,
    /// `[seedling, young, mature, dead]`, like the census rows.
    pub stages: [u32; STAGE_COUNT],
    /// Mean of each locus over living members, in `[0, 1]`; zeros when extinct.
    pub mean_genes: [f32; PlantGenes::BYTE_LEN],
    /// Distinct biomes (by chunk-centre biome) holding a living member.
    pub biomes: Vec<&'static str>,
}

impl StrainReport {
    pub fn extinct(&self) -> bool {
        self.living == 0
    }
}

const ADJECTIVES: [&str; 32] = [
    "Amber", "Ashen", "Bramble", "Cinder", "Copper", "Dawn", "Dusk", "Ember", "Fern", "Frost",
    "Gilded", "Hollow", "Ivory", "Juniper", "Lantern", "Marsh", "Mist", "Moss", "Night", "Pale",
    "Quartz", "Ridge", "Rust", "Sable", "Silver", "Slate", "Storm", "Sun", "Thistle", "Tide",
    "Willow", "Wren",
];

const NUMERALS: [&str; 9] = ["II", "III", "IV", "V", "VI", "VII", "VIII", "IX", "X"];

/// A readable name like "Ridge Pine" (or "Ridge × Pine" for a hybrid), with a
/// numeral suffix when the name is taken. `pick` is any per-strain hash.
pub fn strain_name(pick: u32, species_name: &str, hybrid: bool, taken: &[Strain]) -> String {
    let adjective = ADJECTIVES[pick as usize % ADJECTIVES.len()];
    let base = if hybrid {
        format!("{adjective} × {species_name}")
    } else {
        format!("{adjective} {species_name}")
    };
    let is_free = |name: &str| !taken.iter().any(|s| s.name == name);
    if is_free(&base) {
        return base;
    }
    NUMERALS
        .iter()
        .map(|n| format!("{base} {n}"))
        .find(|name| is_free(name))
        .unwrap_or_else(|| format!("{base} #{}", taken.len() + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strain(name: &str) -> Strain {
        Strain {
            id: 1,
            name: name.to_string(),
            species: 0,
            hybrid: false,
            founded_hour: 0.0,
            founder: StrainFounder {
                chunk: 0,
                local_x: 0,
                local_z: 0,
            },
        }
    }

    #[test]
    fn names_are_unique_with_numeral_suffixes() {
        let first = strain_name(21, "Pine", false, &[]);
        assert_eq!(first, "Ridge Pine");
        let taken = vec![strain(&first)];
        assert_eq!(strain_name(21, "Pine", false, &taken), "Ridge Pine II");
        assert_eq!(strain_name(21, "Pine", true, &taken), "Ridge × Pine");
    }
}
