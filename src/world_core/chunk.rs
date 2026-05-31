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

impl ChunkTerrain {
    /// Sample the terrain height at a position local to this chunk, where
    /// `local_x`/`local_z` are in meters within `[0, CHUNK_SIZE_METERS]`.
    /// Bilinearly interpolates the stored `CHUNK_GRID_RESOLUTION²` height grid.
    pub fn height_at_world(&self, local_x: f32, local_z: f32) -> f32 {
        let side = CHUNK_GRID_RESOLUTION;
        let cell = CHUNK_SIZE_METERS / (side - 1) as f32;

        let max_idx = (side - 1) as f32;
        let fx = (local_x / cell).clamp(0.0, max_idx);
        let fz = (local_z / cell).clamp(0.0, max_idx);

        let x0 = fx.floor() as usize;
        let z0 = fz.floor() as usize;
        let x1 = (x0 + 1).min(side - 1);
        let z1 = (z0 + 1).min(side - 1);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;

        let h = |x: usize, z: usize| self.heights[z * side + x];
        let top = h(x0, z0) * (1.0 - tx) + h(x1, z0) * tx;
        let bottom = h(x0, z1) * (1.0 - tx) + h(x1, z1) * tx;
        top * (1.0 - tz) + bottom * tz
    }
}

#[derive(Clone)]
pub struct ChunkData {
    pub coord: IVec2,
    pub terrain: ChunkTerrain,
    pub content: ChunkContent,
}
