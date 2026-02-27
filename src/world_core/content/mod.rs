mod flora;

use self::flora::{FloraInput, FloraLayer};

use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{ChunkContent, ChunkTerrain};
use crate::world_core::layer::Layer;

pub struct ContentInput<'a> {
    pub coord: glam::IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

pub struct ContentLayer {
    flora: FloraLayer,
}

impl ContentLayer {
    pub fn new(seed: u32) -> Self {
        Self {
            flora: FloraLayer::new(seed),
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
        }
    }
}
