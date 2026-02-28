use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GameConfig {
    pub world: WorldConfig,
    pub sea_level: f32,
    pub biome: BiomeConfig,
    pub heightmap: HeightmapConfig,
    pub flora: FloraConfig,
    pub ferns: FernsConfig,
    pub houses: HousesConfig,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            world: WorldConfig::default(),
            sea_level: 18.0,
            biome: BiomeConfig::default(),
            heightmap: HeightmapConfig::default(),
            flora: FloraConfig::default(),
            ferns: FernsConfig::default(),
            houses: HousesConfig::default(),
        }
    }
}

impl GameConfig {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load() -> Self {
        let path = std::path::Path::new("config.json");
        if !path.exists() {
            log::info!("no config.json found, using defaults");
            return Self::default();
        }
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(config) => {
                    log::info!("loaded config.json");
                    config
                }
                Err(e) => {
                    log::warn!("failed to parse config.json: {e}, using defaults");
                    Self::default()
                }
            },
            Err(e) => {
                log::warn!("failed to read config.json: {e}, using defaults");
                Self::default()
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            load_radius: 1,
            start_hour: 9.5,
            day_speed: 0.04,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct NoiseLayer {
    pub frequency: f64,
    pub amplitude: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FloraConfig {
    pub grid_spacing: f32,
    pub forest_density_base: f32,
    pub forest_density_scale: f32,
    pub forest_density_min: f32,
    pub forest_density_max: f32,
    pub forest_moisture_center: f32,
    pub grassland_density_base: f32,
    pub grassland_density_scale: f32,
    pub grassland_density_min: f32,
    pub grassland_density_max: f32,
    pub trunk_height_min: f32,
    pub trunk_height_range: f32,
    pub canopy_radius_min: f32,
    pub canopy_radius_range: f32,
    pub max_slope: f32,
    pub min_height: f32,
}

impl Default for FloraConfig {
    fn default() -> Self {
        Self {
            grid_spacing: 11.0,
            forest_density_base: 0.42,
            forest_density_scale: 0.7,
            forest_density_min: 0.30,
            forest_density_max: 0.72,
            forest_moisture_center: 0.62,
            grassland_density_base: 0.02,
            grassland_density_scale: 0.08,
            grassland_density_min: 0.01,
            grassland_density_max: 0.11,
            trunk_height_min: 4.5,
            trunk_height_range: 7.5,
            canopy_radius_min: 1.7,
            canopy_radius_range: 2.5,
            max_slope: 1.0,
            min_height: -20.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FernsConfig {
    pub grid_spacing: f32,
    pub forest_density_offset: f32,
    pub forest_density_scale: f32,
    pub forest_density_max: f32,
    pub grassland_density_offset: f32,
    pub grassland_density_scale: f32,
    pub grassland_density_max: f32,
    pub scale_min: f32,
    pub scale_range: f32,
    pub max_slope: f32,
    pub min_height: f32,
}

impl Default for FernsConfig {
    fn default() -> Self {
        Self {
            grid_spacing: 5.0,
            forest_density_offset: 0.55,
            forest_density_scale: 1.5,
            forest_density_max: 0.6,
            grassland_density_offset: 0.5,
            grassland_density_scale: 0.15,
            grassland_density_max: 0.05,
            scale_min: 0.7,
            scale_range: 0.7,
            max_slope: 0.8,
            min_height: -20.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
