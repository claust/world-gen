use crate::world_core::biome::{classify, Biome};
use crate::world_core::chunk::{ChunkTerrain, CHUNK_GRID_RESOLUTION};
use crate::world_core::layer::Layer;

#[derive(Clone)]
pub struct BiomeMap {
    pub values: Vec<Biome>,
}

pub struct BiomeLayer;

impl Layer<&ChunkTerrain, BiomeMap> for BiomeLayer {
    fn generate(&self, terrain: &ChunkTerrain) -> BiomeMap {
        let total = CHUNK_GRID_RESOLUTION * CHUNK_GRID_RESOLUTION;
        if terrain.heights.len() != total || terrain.moisture.len() != total {
            return BiomeMap { values: Vec::new() };
        }

        let values = terrain
            .heights
            .iter()
            .zip(terrain.moisture.iter())
            .map(|(height, moisture)| classify(*height, *moisture))
            .collect();

        BiomeMap { values }
    }
}
