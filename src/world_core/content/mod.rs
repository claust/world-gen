mod flora;
mod houses;
pub(crate) mod sampling;

use self::flora::{FloraInput, FloraLayer};
use self::houses::{HousesInput, HousesLayer};

use std::sync::Arc;

use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{ChunkContent, ChunkTerrain};
use crate::world_core::config::GameConfig;
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::layer::Layer;

pub struct ContentInput<'a> {
    pub coord: glam::IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

pub struct ContentLayer {
    flora: FloraLayer,
    houses: HousesLayer,
}

impl ContentLayer {
    pub fn new(seed: u32, config: &GameConfig, registry: Arc<PlantRegistry>) -> Self {
        Self {
            flora: FloraLayer::new(seed, config.sea_level, registry),
            houses: HousesLayer::new(seed, config.houses.clone(), config.sea_level),
        }
    }
}

impl<'a> Layer<ContentInput<'a>, ChunkContent> for ContentLayer {
    fn generate(&self, input: ContentInput<'a>) -> ChunkContent {
        let base_plants = self.flora.generate(FloraInput {
            coord: input.coord,
            terrain: input.terrain,
            biome_map: input.biome_map,
        });

        ChunkContent {
            plants: base_plants.clone(),
            base_plants,
            plants_revision: 0,
            houses: self.houses.generate(HousesInput {
                coord: input.coord,
                terrain: input.terrain,
                biome_map: input.biome_map,
            }),
        }
    }
}
