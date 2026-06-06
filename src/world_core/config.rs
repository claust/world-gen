use serde::{Deserialize, Serialize};

use super::storage::Storage;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameConfig {
    pub world: WorldConfig,
    pub sea_level: f32,
    pub biome: BiomeConfig,
    pub heightmap: HeightmapConfig,
    pub rivers: RiverConfig,
    pub houses: HousesConfig,
    pub audio: AudioConfig,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            world: WorldConfig::default(),
            sea_level: 40.0,
            biome: BiomeConfig::default(),
            heightmap: HeightmapConfig::default(),
            rivers: RiverConfig::default(),
            houses: HousesConfig::default(),
            audio: AudioConfig::default(),
        }
    }
}

pub const NATIVE_DEFAULT_RIVER_GRID_RESOLUTION: u32 = 2048;
pub const WEB_DEFAULT_RIVER_GRID_RESOLUTION: u32 = 1024;

/// Parameters for the global river field (`world_core::rivers`). These feed the
/// base-generation key, so changing any of them invalidates a cached base world
/// and plants are re-placed on the new river terrain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RiverConfig {
    /// Master switch for river carving.
    pub enabled: bool,
    /// Grid nodes per world side for the hydrology solve. Higher = finer
    /// (narrower) rivers but a longer one-time solve at world load. 2048 ≈
    /// 32 m/node, 1024 ≈ 64 m/node. The default is lower on wasm, where the
    /// solve runs single-threaded.
    pub grid_resolution: u32,
    /// Box-blur radius (cells) applied to the routing surface so flow
    /// concentrates into trunk rivers instead of fragmenting on detail noise.
    pub smooth_radius: u32,
    /// Box-blur iterations (more ≈ a wider gaussian).
    pub smooth_iters: u32,
    /// Upstream drainage area (m²) a cell must collect before it becomes a
    /// river. Smaller = denser network. Resolution-independent.
    pub min_drainage_area_m2: f32,
    /// Channel depth (metres) carved for the largest rivers; smaller rivers
    /// carve proportionally less.
    pub max_carve_depth: f32,
}

impl Default for RiverConfig {
    fn default() -> Self {
        // The solve is parallelized with rayon natively but runs single-threaded
        // on wasm, so default to a coarser grid there to keep browser world-load
        // time reasonable. The release workflow generates a web-profile
        // `world_base.bin` with this same value so downloaded snapshots validate.
        #[cfg(not(target_arch = "wasm32"))]
        let grid_resolution = NATIVE_DEFAULT_RIVER_GRID_RESOLUTION;
        #[cfg(target_arch = "wasm32")]
        let grid_resolution = WEB_DEFAULT_RIVER_GRID_RESOLUTION;
        Self {
            enabled: true,
            grid_resolution,
            smooth_radius: 2,
            smooth_iters: 2,
            min_drainage_area_m2: 150_000.0,
            max_carve_depth: 14.0,
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

/// Ambient sound mix. Per-layer gains multiply `master_volume`; all in `0..1`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    pub enabled: bool,
    pub master_volume: f32,
    pub bird_volume: f32,
    pub sea_volume: f32,
    pub underwater_volume: f32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            master_volume: 1.0,
            bird_volume: 1.0,
            sea_volume: 1.0,
            underwater_volume: 1.0,
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
                frequency: 0.0001,
                amplitude: 180.0,
            },
            ridge: NoiseLayer {
                frequency: 0.0009,
                amplitude: 45.0,
            },
            detail: NoiseLayer {
                frequency: 0.018,
                amplitude: 6.0,
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
    pub hamlet_spacing: f32,
    pub hamlet_density: f32,
    pub hamlet_house_min: u32,
    pub hamlet_house_max: u32,
    pub hamlet_radius: f32,
}

impl Default for HousesConfig {
    fn default() -> Self {
        Self {
            grid_spacing: 40.0,
            grassland_density: 0.01,
            max_slope: 0.3,
            height_min: 0.0,
            height_max: 100.0,
            hamlet_spacing: 100.0,
            hamlet_density: 0.03,
            hamlet_house_min: 3,
            hamlet_house_max: 5,
            hamlet_radius: 15.0,
        }
    }
}
