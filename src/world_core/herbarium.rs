use serde::{Deserialize, Serialize};

use super::plant_gen::config::SpeciesConfig;

#[derive(Clone, Serialize, Deserialize)]
pub struct HerbariumEntry {
    pub name: String,
    pub species: SpeciesConfig,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Herbarium {
    pub plants: Vec<HerbariumEntry>,
}

const SPECIES_PRESETS: &[(&str, &str)] = &[
    ("Oak", include_str!("plant_gen/species/oak.json")),
    ("Birch", include_str!("plant_gen/species/birch.json")),
    ("Acacia", include_str!("plant_gen/species/acacia.json")),
    ("Palm", include_str!("plant_gen/species/palm.json")),
    ("Shrub", include_str!("plant_gen/species/shrub.json")),
    ("Spruce", include_str!("plant_gen/species/spruce.json")),
    ("Willow", include_str!("plant_gen/species/willow.json")),
];

impl Herbarium {
    pub fn default_seeded() -> Self {
        let plants = SPECIES_PRESETS
            .iter()
            .map(|(name, json)| {
                let species: SpeciesConfig = serde_json::from_str(json)
                    .unwrap_or_else(|e| panic!("invalid {name}.json: {e}"));
                HerbariumEntry {
                    name: name.to_string(),
                    species,
                }
            })
            .collect();
        Self { plants }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Herbarium {
    pub fn load() -> Self {
        let path = std::path::Path::new("herbarium.json");
        if !path.exists() {
            let herb = Self::default_seeded();
            if let Err(e) = herb.save() {
                log::warn!("failed to write initial herbarium.json: {e}");
            }
            return herb;
        }
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(herb) => {
                    log::info!("loaded herbarium.json");
                    herb
                }
                Err(e) => {
                    log::warn!("failed to parse herbarium.json: {e}");
                    Self::default_seeded()
                }
            },
            Err(e) => {
                log::warn!("failed to read herbarium.json: {e}");
                Self::default_seeded()
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write("herbarium.json", json)?;
        log::info!("saved herbarium.json");
        Ok(())
    }
}
