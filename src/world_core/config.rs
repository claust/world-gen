use serde::{Deserialize, Serialize};

use super::storage::Storage;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameConfig {
    pub world: WorldConfig,
    pub sea_level: f32,
    pub biome: BiomeConfig,
    pub heightmap: HeightmapConfig,
    pub houses: HousesConfig,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            world: WorldConfig::default(),
            sea_level: 40.0,
            biome: BiomeConfig::default(),
            heightmap: HeightmapConfig::default(),
            houses: HousesConfig::default(),
        }
    }
}

impl GameConfig {
    pub fn load(storage: &dyn Storage) -> Self {
        match storage.load("config") {
            Some(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("loaded config");
                    config
                }
                Err(e) => {
                    log::warn!("failed to parse config: {e}, using defaults");
                    Self::default()
                }
            },
            None => {
                log::info!("no config found, using defaults");
                Self::default()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldConfig {
    pub seed: u32,
    pub load_radius: i32,
    pub start_hour: f32,
    pub day_speed: f32,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            seed: 42,
            load_radius: 3,
            start_hour: 5.5,
            day_speed: 0.04,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BiomeConfig {
    pub snow_height: f32,
    pub rock_height: f32,
    pub desert_moisture: f32,
    pub forest_moisture: f32,
}

impl Default for BiomeConfig {
    fn default() -> Self {
        Self {
            snow_height: 165.0,
            rock_height: 120.0,
            desert_moisture: 0.3,
            forest_moisture: 0.62,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct NoiseLayer {
    pub frequency: f64,
    pub amplitude: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeightmapConfig {
    pub continental: NoiseLayer,
    pub ridge: NoiseLayer,
    pub detail: NoiseLayer,
    pub moisture_base_frequency: f64,
    pub moisture_variation_frequency: f64,
    pub moisture_base_weight: f32,
    pub moisture_variation_weight: f32,
    pub moisture_variation_offset_x: f64,
    pub moisture_variation_offset_z: f64,
}

impl Default for HeightmapConfig {
    fn default() -> Self {
        Self {
            continental: NoiseLayer {
                frequency: 0.0013,
                amplitude: 140.0,
            },
            ridge: NoiseLayer {
                frequency: 0.0032,
                amplitude: 65.0,
            },
            detail: NoiseLayer {
                frequency: 0.018,
                amplitude: 10.0,
            },
            moisture_base_frequency: 0.0019,
            moisture_variation_frequency: 0.0095,
            moisture_base_weight: 0.75,
            moisture_variation_weight: 0.25,
            moisture_variation_offset_x: 31.0,
            moisture_variation_offset_z: -11.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HousesConfig {
    pub grid_spacing: f32,
    pub grassland_density: f32,
    pub max_slope: f32,
    pub height_min: f32,
    pub height_max: f32,
}

impl Default for HousesConfig {
    fn default() -> Self {
        Self {
            grid_spacing: 40.0,
            grassland_density: 0.04,
            max_slope: 0.3,
            height_min: 0.0,
            height_max: 100.0,
        }
    }
}
