pub mod config;
pub mod crown;
pub mod mesh;
pub mod sdf;
pub mod tree;

use config::SpeciesConfig;
use mesh::build_mesh;
use tree::{compact_foliage, generate_tree};

#[derive(Clone, Debug)]
pub struct PlantVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

pub struct PlantMesh {
    pub vertices: Vec<PlantVertex>,
    pub indices: Vec<u32>,
}

pub fn generate_plant_mesh(spec: &SpeciesConfig, seed: u32) -> PlantMesh {
    let mut tree_data = generate_tree(spec, seed);
    compact_foliage(&mut tree_data.foliage);
    let (vertices, indices) = build_mesh(spec, &tree_data);
    PlantMesh { vertices, indices }
}
