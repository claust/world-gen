mod ferns;
mod flora;
mod houses;
mod sampling;

use self::ferns::{FernsInput, FernsLayer};
use self::flora::{FloraInput, FloraLayer};
use self::houses::{HousesInput, HousesLayer};

use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{ChunkContent, ChunkTerrain};
use crate::world_core::config::GameConfig;
use crate::world_core::layer::Layer;

pub struct ContentInput<'a> {
    pub coord: glam::IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

pub struct ContentLayer {
    flora: FloraLayer,
    houses: HousesLayer,
    ferns: FernsLayer,
}

impl ContentLayer {
    pub fn new(seed: u32, config: &GameConfig) -> Self {
        Self {
            flora: FloraLayer::new(seed, config.flora.clone(), config.sea_level),
            houses: HousesLayer::new(seed, config.houses.clone(), config.sea_level),
            ferns: FernsLayer::new(seed, config.ferns.clone(), config.sea_level),
        }
    }
}

impl<'a> Layer<ContentInput<'a>, ChunkContent> for ContentLayer {
    fn generate(&self, input: ContentInput<'a>) -> ChunkContent {
        ChunkContent {
            trees: self.flora.generate(FloraInput {
                coord: input.coord,
                terrain: input.terrain,
                biome_map: input.biome_map,
            }),
            houses: self.houses.generate(HousesInput {
                coord: input.coord,
                terrain: input.terrain,
                biome_map: input.biome_map,
            }),
            ferns: self.ferns.generate(FernsInput {
                coord: input.coord,
                terrain: input.terrain,
                biome_map: input.biome_map,
            }),
        }
    }
}
