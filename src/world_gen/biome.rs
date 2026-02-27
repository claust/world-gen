use glam::Vec3;

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

pub fn ground_color(biome: Biome, height: f32) -> Vec3 {
    let base = match biome {
        Biome::Snow => Vec3::new(0.90, 0.92, 0.95),
        Biome::Rock => Vec3::new(0.46, 0.48, 0.50),
        Biome::Desert => Vec3::new(0.70, 0.60, 0.36),
        Biome::Forest => Vec3::new(0.21, 0.43, 0.23),
        Biome::Grassland => Vec3::new(0.34, 0.52, 0.24),
    };

    let tint = ((height + 40.0) / 260.0).clamp(0.0, 1.0);
    base.lerp(Vec3::splat(0.75), tint * 0.08)
}
