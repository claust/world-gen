#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Biome {
    Snow,
    Rock,
    Desert,
    Forest,
    Grassland,
}

pub fn classify(height: f32, moisture: f32) -> Biome {
    if height > 165.0 {
        return Biome::Snow;
    }
    if height > 120.0 {
        return Biome::Rock;
    }
    if moisture < 0.3 {
        return Biome::Desert;
    }
    if moisture > 0.62 {
        return Biome::Forest;
    }
    Biome::Grassland
}
