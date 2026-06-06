use glam::IVec2;

use std::sync::Arc;

use crate::world_core::biome_map::BiomeLayer;
use crate::world_core::chunk::ChunkData;
use crate::world_core::config::GameConfig;
use crate::world_core::content::{ContentInput, ContentLayer};
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::layer::Layer;
use crate::world_core::rivers::RiverField;
use crate::world_core::terrain::TerrainLayer;

pub struct ChunkGenerator {
    terrain_layer: TerrainLayer,
    biome_layer: BiomeLayer,
    content_layer: ContentLayer,
}

impl ChunkGenerator {
    pub fn new(
        seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
    ) -> Self {
        Self {
            terrain_layer: TerrainLayer::new(
                seed,
                config.heightmap.clone(),
                config.sea_level,
                rivers,
            ),
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
    use crate::world_core::rivers::RiverField;
    use glam::IVec2;
    use std::sync::Arc;

    #[test]
    fn wrapped_chunks_generate_identical_content_offset_by_one_lap() {
        use crate::world_core::chunk::{CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS};

        let config = GameConfig::default();
        let herb = Herbarium::default_seeded();
        let rivers = Arc::new(RiverField::generate(7, &config));
        let generator = ChunkGenerator::new(
            7,
            &config,
            Arc::new(PlantRegistry::from_herbarium(&herb)),
            rivers,
        );

        // A chunk and the same chunk one full world lap east must share terrain
        // and content; only the world-space position is shifted by `L`.
        let raw = IVec2::new(5, -3);
        let wrapped = IVec2::new(raw.x + WORLD_SIZE_CHUNKS, raw.y);
        let lap = WORLD_SIZE_CHUNKS as f32 * CHUNK_SIZE_METERS;

        let a = generator.generate_chunk(raw);
        let b = generator.generate_chunk(wrapped);

        // Terrain field is bit-for-bit identical (periodic noise, position-free).
        assert_eq!(a.terrain.heights, b.terrain.heights);
        assert_eq!(a.terrain.moisture, b.terrain.moisture);

        // Same vegetation, shifted one lap in x.
        assert_eq!(a.content.base_plants.len(), b.content.base_plants.len());
        for (pa, pb) in a
            .content
            .base_plants
            .iter()
            .zip(b.content.base_plants.iter())
        {
            assert!((pb.position.x - pa.position.x - lap).abs() < 1e-2);
            assert!((pb.position.z - pa.position.z).abs() < 1e-2);
            assert!((pa.position.y - pb.position.y).abs() < 1e-3);
            assert_eq!(pa.species_index, pb.species_index);
            assert!((pa.height - pb.height).abs() < 1e-3);
        }

        // Same houses, shifted one lap in x.
        assert_eq!(a.content.houses.len(), b.content.houses.len());
        for (ha, hb) in a.content.houses.iter().zip(b.content.houses.iter()) {
            assert!((hb.position.x - ha.position.x - lap).abs() < 1e-2);
            assert!((hb.position.z - ha.position.z).abs() < 1e-2);
        }
    }

    #[test]
    fn plant_generation_is_deterministic_for_same_seed_and_chunk() {
        let config = GameConfig::default();
        let herb = Herbarium::default_seeded();
        let reg_a = Arc::new(PlantRegistry::from_herbarium(&herb));
        let reg_b = Arc::new(PlantRegistry::from_herbarium(&herb));
        let rivers = Arc::new(RiverField::generate(42, &config));
        let a = ChunkGenerator::new(42, &config, reg_a, Arc::clone(&rivers))
            .generate_chunk(IVec2::new(3, -2));
        let b = ChunkGenerator::new(42, &config, reg_b, rivers).generate_chunk(IVec2::new(3, -2));

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
