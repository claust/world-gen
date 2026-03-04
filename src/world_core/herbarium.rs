use serde::{Deserialize, Serialize};

pub use super::plant_gen::config::SpeciesConfig;
use super::storage::Storage;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlacementConfig {
    pub biomes: Vec<String>,
    pub weight: f32,
    pub min_moisture: f32,
    pub max_moisture: f32,
    pub min_altitude: f32,
    pub max_altitude: f32,
    pub near_water_boost: f32,
    pub max_slope: f32,
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            biomes: vec!["Forest".to_string(), "Grassland".to_string()],
            weight: 1.0,
            min_moisture: 0.0,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 200.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct HerbariumEntry {
    pub name: String,
    pub species: SpeciesConfig,
    #[serde(default)]
    pub placement: PlacementConfig,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Herbarium {
    pub plants: Vec<HerbariumEntry>,
}

/// A deduplicated, indexed view of the herbarium for world generation.
/// Each species appears once; `species_index` values in `PlantInstance` refer
/// to indices in `species`.
pub struct PlantRegistry {
    pub species: Vec<PlantSpeciesInfo>,
}

/// Flattened info for one species used during placement and rendering.
pub struct PlantSpeciesInfo {
    pub name: String,
    pub kind: String,
    pub crown_shape: String,
    pub crown_base: f32,
    pub aspect_ratio: f32,
    pub trunk_thickness_ratio: f32,
    pub trunk_taper: f32,
    pub bark_color: [f32; 3],
    pub leaf_color: [f32; 3],
    pub height_range: [f32; 2],
    pub placement: PlacementConfig,
    /// Full species config for procedural mesh generation.
    pub species_config: SpeciesConfig,
}

impl PlantRegistry {
    pub fn from_herbarium(herb: &Herbarium) -> Self {
        let mut species = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for entry in &herb.plants {
            if !seen_names.insert(entry.name.clone()) {
                log::warn!("skipping duplicate herbarium entry: {}", entry.name);
                continue;
            }

            let bark = crate::world_core::color::hsl_to_linear(
                entry.species.color.bark.h,
                entry.species.color.bark.s,
                entry.species.color.bark.l,
            );
            let leaf = crate::world_core::color::hsl_to_linear(
                entry.species.color.leaf.h,
                entry.species.color.leaf.s,
                entry.species.color.leaf.l,
            );

            species.push(PlantSpeciesInfo {
                name: entry.name.clone(),
                kind: entry.species.body_plan.kind.clone(),
                crown_shape: entry.species.crown.shape.clone(),
                crown_base: entry.species.crown.crown_base,
                aspect_ratio: entry.species.crown.aspect_ratio,
                trunk_thickness_ratio: entry.species.trunk.thickness_ratio,
                trunk_taper: entry.species.trunk.taper,
                bark_color: bark,
                leaf_color: leaf,
                height_range: entry.species.body_plan.max_height,
                placement: entry.placement.clone(),
                species_config: entry.species.clone(),
            });
        }

        Self { species }
    }
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

fn default_placement(name: &str) -> PlacementConfig {
    match name {
        "Oak" => PlacementConfig {
            biomes: vec!["Forest".into(), "Grassland".into()],
            weight: 1.0,
            min_moisture: 0.3,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 120.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
        },
        "Birch" => PlacementConfig {
            biomes: vec!["Forest".into()],
            weight: 0.7,
            min_moisture: 0.5,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 120.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
        },
        "Acacia" => PlacementConfig {
            biomes: vec!["Desert".into(), "Grassland".into()],
            weight: 0.6,
            min_moisture: 0.0,
            max_moisture: 0.5,
            min_altitude: 0.0,
            max_altitude: 100.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
        },
        "Palm" => PlacementConfig {
            biomes: vec!["Grassland".into(), "Forest".into()],
            weight: 0.5,
            min_moisture: 0.4,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 60.0,
            near_water_boost: 0.5,
            max_slope: 1.0,
        },
        "Shrub" => PlacementConfig {
            biomes: vec!["Forest".into(), "Grassland".into()],
            weight: 1.0,
            min_moisture: 0.2,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 120.0,
            near_water_boost: 0.0,
            max_slope: 0.8,
        },
        "Spruce" => PlacementConfig {
            biomes: vec!["Forest".into()],
            weight: 0.8,
            min_moisture: 0.3,
            max_moisture: 1.0,
            min_altitude: 60.0,
            max_altitude: 160.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
        },
        "Willow" => PlacementConfig {
            biomes: vec!["Forest".into(), "Grassland".into()],
            weight: 0.4,
            min_moisture: 0.5,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 80.0,
            near_water_boost: 0.7,
            max_slope: 1.0,
        },
        _ => PlacementConfig::default(),
    }
}

impl Herbarium {
    /// Create a new herbarium entry with Oak defaults and the given name.
    pub fn new_entry(name: String) -> HerbariumEntry {
        let mut species: SpeciesConfig =
            serde_json::from_str(SPECIES_PRESETS[0].1).expect("invalid oak.json");
        species.name = name.clone();
        HerbariumEntry {
            name,
            species,
            placement: PlacementConfig::default(),
        }
    }

    pub fn default_seeded() -> Self {
        let plants = SPECIES_PRESETS
            .iter()
            .map(|(name, json)| {
                let species: SpeciesConfig = serde_json::from_str(json)
                    .unwrap_or_else(|e| panic!("invalid {name}.json: {e}"));
                HerbariumEntry {
                    name: name.to_string(),
                    placement: default_placement(name),
                    species,
                }
            })
            .collect();
        Self { plants }
    }
}

impl Herbarium {
    pub fn load(storage: &dyn Storage) -> Self {
        match storage.load("herbarium") {
            Some(contents) => match serde_json::from_str(&contents) {
                Ok(herb) => {
                    log::info!("loaded herbarium");
                    herb
                }
                Err(e) => {
                    log::warn!("failed to parse herbarium: {e}");
                    Self::default_seeded()
                }
            },
            None => {
                let herb = Self::default_seeded();
                if let Err(e) = herb.save(storage) {
                    log::warn!("failed to write initial herbarium: {e}");
                }
                herb
            }
        }
    }

    pub fn save(&self, storage: &dyn Storage) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        storage.save("herbarium", &json)?;
        log::info!("saved herbarium");
        Ok(())
    }
}
