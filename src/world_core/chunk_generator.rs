use glam::IVec2;

use std::sync::Arc;

use crate::world_core::biome_map::BiomeLayer;
use crate::world_core::chunk::ChunkData;
use crate::world_core::config::GameConfig;
use crate::world_core::content::{ContentInput, ContentLayer};
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::layer::Layer;
use crate::world_core::terrain::TerrainLayer;

pub struct ChunkGenerator {
    terrain_layer: TerrainLayer,
    biome_layer: BiomeLayer,
    content_layer: ContentLayer,
}

impl ChunkGenerator {
    pub fn new(seed: u32, config: &GameConfig, registry: Arc<PlantRegistry>) -> Self {
        Self {
            terrain_layer: TerrainLayer::new(seed, config.heightmap.clone(), config.sea_level),
            biome_layer: BiomeLayer {
                biome_config: config.biome.clone(),
            },
            content_layer: ContentLayer::new(seed, config, registry),
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
            content,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkGenerator;
    use crate::world_core::config::GameConfig;
    use crate::world_core::herbarium::{Herbarium, PlantRegistry};
    use crate::world_core::lifecycle::GrowthStage;
    use glam::IVec2;
    use std::sync::Arc;

    #[test]
    fn plant_generation_is_deterministic_for_same_seed_and_chunk() {
        let config = GameConfig::default();
        let herb = Herbarium::default_seeded();
        let reg_a = Arc::new(PlantRegistry::from_herbarium(&herb));
        let reg_b = Arc::new(PlantRegistry::from_herbarium(&herb));
        let a = ChunkGenerator::new(42, &config, reg_a).generate_chunk(IVec2::new(3, -2));
        let b = ChunkGenerator::new(42, &config, reg_b).generate_chunk(IVec2::new(3, -2));

        assert_eq!(a.content.base_plants.len(), b.content.base_plants.len());
        assert_eq!(a.content.base_plants.len(), a.content.plants.len());
        assert_eq!(b.content.base_plants.len(), b.content.plants.len());
        for (pa, pb) in a
            .content
            .base_plants
            .iter()
            .zip(b.content.base_plants.iter())
        {
            assert!((pa.position - pb.position).length() < 1e-5);
            assert!((pa.height - pb.height).abs() < 1e-5);
            assert_eq!(pa.species_index, pb.species_index);
            assert_eq!(pa.growth_stage, GrowthStage::Mature);
            assert_eq!(pb.growth_stage, GrowthStage::Mature);
        }
    }
}
