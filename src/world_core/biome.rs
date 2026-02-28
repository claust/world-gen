use crate::world_core::config::BiomeConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biome {
    Snow,
    Rock,
    Desert,
    Forest,
    Grassland,
}

pub fn classify(height: f32, moisture: f32, config: &BiomeConfig) -> Biome {
    if height > config.snow_height {
        return Biome::Snow;
    }
    if height > config.rock_height {
        return Biome::Rock;
    }
    if moisture < config.desert_moisture {
        return Biome::Desert;
    }
    if moisture > config.forest_moisture {
        return Biome::Forest;
    }
    Biome::Grassland
}
