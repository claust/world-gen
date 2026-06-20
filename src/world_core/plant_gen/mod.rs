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
    pub parts: Vec<PlantMeshPart>,
}

impl PlantMesh {
    pub fn indices_for_part(&self, kind: PlantMeshPartKind) -> Vec<u32> {
        self.parts
            .iter()
            .filter(|part| part.kind == kind)
            .flat_map(|part| {
                let start = part.index_start as usize;
                let end = start.saturating_add(part.index_count as usize);
                self.indices
                    .get(start..end)
                    .into_iter()
                    .flat_map(|indices| indices.iter().copied())
            })
            .collect()
    }

    pub fn opaque_indices(&self) -> Vec<u32> {
        self.indices_for_part(PlantMeshPartKind::Opaque)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlantMeshPartKind {
    Opaque,
    FoliageCards,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlantMeshPart {
    pub kind: PlantMeshPartKind,
    pub vertex_start: u32,
    pub vertex_count: u32,
    pub index_start: u32,
    pub index_count: u32,
}

pub fn generate_plant_mesh(spec: &SpeciesConfig, seed: u32) -> PlantMesh {
    let mut tree_data = generate_tree(spec, seed);
    compact_foliage(&mut tree_data.foliage);
    build_mesh(spec, &tree_data)
}

#[cfg(test)]
mod tests {
    use super::config::{LeafType, SpeciesConfig};
    use super::tree::{compact_foliage, generate_tree, SdfFoliageKind};
    use super::{generate_plant_mesh, PlantMeshPartKind};

    fn species(json: &str) -> SpeciesConfig {
        serde_json::from_str(json).expect("species preset should deserialize")
    }

    #[test]
    fn broadleaf_uses_sdf_and_needle_uses_cards() {
        let oak = species(include_str!("species/oak.json"));
        let spruce = species(include_str!("species/spruce.json"));

        let mut oak_tree = generate_tree(&oak, 7);
        compact_foliage(&mut oak_tree.foliage);
        assert!(!oak_tree.foliage.is_empty());
        assert!(oak_tree
            .sdf_foliage_kinds()
            .all(|kind| kind == SdfFoliageKind::Broadleaf));
        assert_eq!(oak_tree.needle_cards().count(), 0);

        let mut spruce_tree = generate_tree(&spruce, 7);
        compact_foliage(&mut spruce_tree.foliage);
        assert!(!spruce_tree.foliage.is_empty());
        assert_eq!(spruce_tree.sdf_foliage_kinds().count(), 0);
        assert!(spruce_tree.needle_cards().count() > 0);
    }

    #[test]
    fn leafless_species_produce_no_foliage_elements() {
        let oak = species(include_str!("species/oak.json"));
        let dead_oak = oak.deadify();

        let dead_tree = generate_tree(&dead_oak, 11);
        assert!(dead_tree.foliage.is_empty());

        let dead_mesh = generate_plant_mesh(&dead_oak, 11);
        assert!(dead_mesh
            .parts
            .iter()
            .all(|part| part.kind != PlantMeshPartKind::FoliageCards));
    }

    #[test]
    fn generated_needle_mesh_records_opaque_and_card_parts() {
        let spruce = species(include_str!("species/spruce.json"));
        let mesh = generate_plant_mesh(&spruce, 13);

        assert!(!mesh.vertices.is_empty());
        assert!(!mesh.indices.is_empty());
        assert_eq!(mesh.parts.len(), 2);
        assert_eq!(mesh.parts[0].kind, PlantMeshPartKind::Opaque);
        assert_eq!(mesh.parts[0].vertex_start, 0);
        assert!(mesh.parts[0].vertex_count < mesh.vertices.len() as u32);
        assert_eq!(mesh.parts[0].index_start, 0);
        assert!(mesh.parts[0].index_count < mesh.indices.len() as u32);

        assert_eq!(mesh.parts[1].kind, PlantMeshPartKind::FoliageCards);
        assert_eq!(
            mesh.parts[1].vertex_start,
            mesh.parts[0].vertex_start + mesh.parts[0].vertex_count
        );
        assert_eq!(
            mesh.parts[1].index_start,
            mesh.parts[0].index_start + mesh.parts[0].index_count
        );
        assert_eq!(mesh.parts[1].vertex_count % 4, 0);
        assert_eq!(mesh.parts[1].index_count % 6, 0);
        assert_eq!(
            mesh.opaque_indices().len() as u32,
            mesh.parts[0].index_count
        );
    }

    #[test]
    fn broadleaf_mesh_stays_single_opaque_part() {
        let oak = species(include_str!("species/oak.json"));
        let mesh = generate_plant_mesh(&oak, 13);

        assert!(!mesh.vertices.is_empty());
        assert!(!mesh.indices.is_empty());
        assert_eq!(mesh.parts.len(), 1);
        assert_eq!(mesh.parts[0].kind, PlantMeshPartKind::Opaque);
        assert_eq!(mesh.parts[0].vertex_count, mesh.vertices.len() as u32);
        assert_eq!(mesh.parts[0].index_count, mesh.indices.len() as u32);
        assert_eq!(mesh.opaque_indices(), mesh.indices);
    }

    #[test]
    fn malformed_mesh_part_indices_return_empty_slice() {
        let mesh = super::PlantMesh {
            vertices: Vec::new(),
            indices: vec![0, 1, 2],
            parts: vec![super::PlantMeshPart {
                kind: PlantMeshPartKind::Opaque,
                vertex_start: 0,
                vertex_count: 0,
                index_start: 99,
                index_count: 3,
            }],
        };

        assert!(mesh.opaque_indices().is_empty());
    }

    #[test]
    fn needle_fan_top_routes_frond_foliage_to_cards() {
        let mut palm = species(include_str!("species/palm.json"));
        palm.foliage.leaf_type = LeafType::Needle;

        let tree = generate_tree(&palm, 17);
        assert_eq!(tree.sdf_foliage_kinds().count(), 0);
        assert!(tree.needle_cards().count() > 0);
    }
}
