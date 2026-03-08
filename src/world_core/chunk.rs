use glam::{IVec2, Vec3};

use crate::world_core::lifecycle::GrowthStage;

pub const CHUNK_SIZE_METERS: f32 = 256.0;
pub const CHUNK_GRID_RESOLUTION: usize = 129;

/// Global water surface height. Any terrain below this level is submerged.
pub const SEA_LEVEL: f32 = 40.0;

#[derive(Clone, PartialEq)]
pub struct PlantInstance {
    pub position: Vec3,
    pub rotation: f32,
    pub height: f32,
    pub species_index: usize,
    pub growth_stage: GrowthStage,
}

#[derive(Clone)]
pub struct HouseInstance {
    pub position: Vec3,
    pub rotation: f32,
}

#[derive(Clone, Default)]
pub struct ChunkContent {
    pub base_plants: Vec<PlantInstance>,
    pub plants: Vec<PlantInstance>,
    pub plants_revision: u64,
    pub houses: Vec<HouseInstance>,
}

impl ChunkContent {
    pub fn set_plants(&mut self, plants: Vec<PlantInstance>) -> bool {
        if self.plants == plants {
            return false;
        }

        self.plants = plants;
        self.plants_revision = self.plants_revision.wrapping_add(1);
        true
    }
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
