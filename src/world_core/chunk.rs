use glam::{IVec2, Vec3};

use crate::world_core::lifecycle::GrowthStage;

pub const CHUNK_SIZE_METERS: f32 = 256.0;
pub const CHUNK_GRID_RESOLUTION: usize = 129;

/// Side length of the world in chunks. The world is a finite, seamlessly
/// wrapping flat plane (a torus): chunk coordinates wrap every
/// `WORLD_SIZE_CHUNKS` along each axis, so there are only `WORLD_SIZE_CHUNKS²`
/// distinct chunks. World side `L = WORLD_SIZE_CHUNKS * CHUNK_SIZE_METERS`
/// (= 65 536 m at the default).
///
/// This is part of the world format: changing it shifts every canonical chunk
/// id and the noise period, invalidating existing saves. Must stay comfortably
/// larger than the streaming load diameter (`2 * load_radius + 1`) so a single
/// canonical chunk never appears at two raw positions in one frame.
pub const WORLD_SIZE_CHUNKS: i32 = 256;

/// World side length in metres — the period of the wrapping terrain noise.
pub const WORLD_SIZE_METERS: f64 = WORLD_SIZE_CHUNKS as f64 * CHUNK_SIZE_METERS as f64;

/// Global water surface height. Any terrain below this level is submerged.
pub const SEA_LEVEL: f32 = 40.0;

/// Map any (possibly unbounded) raw chunk coordinate to its canonical id in
/// `[0, WORLD_SIZE_CHUNKS)` on both axes. Raw chunk ids an integer number of
/// laps apart collapse to the same canonical id, so they share generated
/// terrain, vegetation, and persisted state — the wrap is invisible because
/// streaming, culling, and placement keep using the raw id, while content
/// generation and lookup go through the canonical id.
pub fn canonical_chunk(coord: IVec2) -> IVec2 {
    IVec2::new(
        coord.x.rem_euclid(WORLD_SIZE_CHUNKS),
        coord.y.rem_euclid(WORLD_SIZE_CHUNKS),
    )
}

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
