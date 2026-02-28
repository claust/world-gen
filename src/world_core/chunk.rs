use glam::{IVec2, Vec3};

use crate::world_core::biome_map::BiomeMap;

pub const CHUNK_SIZE_METERS: f32 = 256.0;
pub const CHUNK_GRID_RESOLUTION: usize = 129;

#[derive(Clone)]
pub struct TreeInstance {
    pub position: Vec3,
    pub trunk_height: f32,
    pub canopy_radius: f32,
}

#[derive(Clone)]
pub struct HouseInstance {
    pub position: Vec3,
    pub rotation: f32,
}

#[derive(Clone, Default)]
pub struct ChunkContent {
    pub trees: Vec<TreeInstance>,
    pub houses: Vec<HouseInstance>,
}

#[derive(Clone)]
pub struct ChunkTerrain {
    pub coord: IVec2,
    pub heights: Vec<f32>,
    pub moisture: Vec<f32>,
    pub min_height: f32,
    pub max_height: f32,
}

#[derive(Clone)]
pub struct ChunkData {
    pub coord: IVec2,
    pub terrain: ChunkTerrain,
    pub biome_map: BiomeMap,
    pub content: ChunkContent,
}
