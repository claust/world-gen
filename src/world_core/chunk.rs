use glam::{IVec2, Vec3};

pub const CHUNK_SIZE_METERS: f32 = 256.0;
pub const CHUNK_GRID_RESOLUTION: usize = 129;

/// Global water surface height. Any terrain below this level is submerged.
pub const SEA_LEVEL: f32 = 40.0;

#[derive(Clone)]
pub struct PlantInstance {
    pub position: Vec3,
    pub rotation: f32,
    pub height: f32,
    pub species_index: u8,
}

#[derive(Clone)]
pub struct HouseInstance {
    pub position: Vec3,
    pub rotation: f32,
}

#[derive(Clone, Default)]
pub struct ChunkContent {
    pub plants: Vec<PlantInstance>,
    pub houses: Vec<HouseInstance>,
}

#[derive(Clone)]
pub struct ChunkTerrain {
    pub heights: Vec<f32>,
    pub moisture: Vec<f32>,
    pub min_height: f32,
    pub max_height: f32,
    /// `true` when any vertex in this chunk is below `SEA_LEVEL`.
    pub has_water: bool,
}

#[derive(Clone)]
pub struct ChunkData {
    pub coord: IVec2,
    pub terrain: ChunkTerrain,
    pub content: ChunkContent,
}
