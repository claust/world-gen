use glam::IVec2;

use crate::world_core::biome_map::BiomeLayer;
use crate::world_core::chunk::ChunkData;
use crate::world_core::content::{ContentInput, ContentLayer};
use crate::world_core::layer::Layer;
use crate::world_core::terrain::TerrainLayer;

pub struct ChunkGenerator {
    terrain_layer: TerrainLayer,
    biome_layer: BiomeLayer,
    content_layer: ContentLayer,
}

impl ChunkGenerator {
    pub fn new(seed: u32) -> Self {
        Self {
            terrain_layer: TerrainLayer::new(seed),
            biome_layer: BiomeLayer,
            content_layer: ContentLayer::new(seed),
        }
    }

    pub fn generate_chunk(&self, coord: IVec2) -> ChunkData {
        let terrain = self.terrain_layer.generate(coord);
        let biome_map = self.biome_layer.generate(&terrain);
        let content = self.content_layer.generate(ContentInput {
            coord,
            terrain: &terrain,
            biome_map: &biome_map,
        });

        ChunkData {
            coord,
            terrain,
            biome_map,
            content,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkGenerator;
    use glam::IVec2;

    #[test]
    fn tree_generation_is_deterministic_for_same_seed_and_chunk() {
        let a = ChunkGenerator::new(42).generate_chunk(IVec2::new(3, -2));
        let b = ChunkGenerator::new(42).generate_chunk(IVec2::new(3, -2));

        assert_eq!(a.content.trees.len(), b.content.trees.len());
        for (ta, tb) in a.content.trees.iter().zip(b.content.trees.iter()) {
            assert!((ta.position - tb.position).length() < 1e-5);
            assert!((ta.trunk_height - tb.trunk_height).abs() < 1e-5);
            assert!((ta.canopy_radius - tb.canopy_radius).abs() < 1e-5);
        }
    }
}
