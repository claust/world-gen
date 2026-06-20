use glam::Vec3;

use super::config::SpeciesConfig;
use super::sdf;
use super::tree::TreeData;
use super::{PlantMesh, PlantMeshPart, PlantMeshPartKind, PlantVertex};
use crate::world_core::color::hsl_to_linear;

const CYL_SIDES: usize = 8;

pub fn build_mesh(spec: &SpeciesConfig, data: &TreeData) -> PlantMesh {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut parts = Vec::new();

    // Bark is the default; a segment may opt into the leaf colour (reed stalks).
    let bark_linear = hsl_to_linear(spec.color.bark.h, spec.color.bark.s, spec.color.bark.l);
    let leaf_linear = hsl_to_linear(spec.color.leaf.h, spec.color.leaf.s, spec.color.leaf.l);

    for seg in &data.segments {
        let base = if seg.use_leaf_color {
            leaf_linear
        } else {
            bark_linear
        };
        let depth_darken = 1.0 - seg.depth as f32 * 0.05;
        let color = [
            base[0] * depth_darken,
            base[1] * depth_darken,
            base[2] * depth_darken,
        ];
        add_cylinder(
            seg.start,
            seg.end,
            seg.start_radius,
            seg.end_radius,
            color,
            &mut vertices,
            &mut indices,
        );
    }

    // Foliage via SDF smooth union + surface nets.
    let foliage_blobs: Vec<_> = data.sdf_blobs().collect();
    if !foliage_blobs.is_empty() {
        let base_idx = vertices.len() as u32;
        let (foliage_verts, foliage_idx) =
            sdf::extract_foliage_surface(&foliage_blobs, &spec.color.leaf);
        vertices.extend(foliage_verts);
        indices.extend(foliage_idx.iter().map(|i| i + base_idx));
    }

    if !indices.is_empty() {
        parts.push(PlantMeshPart {
            kind: PlantMeshPartKind::Opaque,
            vertex_start: 0,
            vertex_count: vertices.len() as u32,
            index_start: 0,
            index_count: indices.len() as u32,
        });
    }

    let card_vertex_start = vertices.len() as u32;
    let card_index_start = indices.len() as u32;
    for card in data.needle_cards() {
        add_needle_card(card, &mut vertices, &mut indices);
    }

    let card_vertex_count = vertices.len() as u32 - card_vertex_start;
    let card_index_count = indices.len() as u32 - card_index_start;
    if card_index_count > 0 {
        parts.push(PlantMeshPart {
            kind: PlantMeshPartKind::FoliageCards,
            vertex_start: card_vertex_start,
            vertex_count: card_vertex_count,
            index_start: card_index_start,
            index_count: card_index_count,
        });
    }

    PlantMesh {
        vertices,
        indices,
        parts,
    }
}

fn add_needle_card(
    card: &super::tree::NeedleCard,
    verts: &mut Vec<PlantVertex>,
    indices: &mut Vec<u32>,
) {
    let base_idx = verts.len() as u32;
    let normal = card
        .axis_u
        .cross(card.axis_v)
        .try_normalize()
        .unwrap_or(Vec3::Y);
    let corners = [
        (card.center - card.axis_u - card.axis_v, [0.0, 0.0]),
        (card.center + card.axis_u - card.axis_v, [1.0, 0.0]),
        (card.center + card.axis_u + card.axis_v, [1.0, 1.0]),
        (card.center - card.axis_u + card.axis_v, [0.0, 1.0]),
    ];

    for (pos, uv) in corners {
        verts.push(PlantVertex {
            position: [pos.x, pos.y, pos.z],
            normal: [normal.x, normal.y, normal.z],
            // Foliage-card shaders read UV from xy and a stable per-card random
            // value from z. Tint jitter lives on the card data for future use.
            color: [uv[0], uv[1], card.random],
        });
    }

    indices.extend_from_slice(&[
        base_idx,
        base_idx + 1,
        base_idx + 2,
        base_idx,
        base_idx + 2,
        base_idx + 3,
    ]);
}

fn add_cylinder(
    start: Vec3,
    end: Vec3,
    start_r: f32,
    end_r: f32,
    color: [f32; 3],
    verts: &mut Vec<PlantVertex>,
    indices: &mut Vec<u32>,
) {
    let base_idx = verts.len() as u32;
    let dir = (end - start).normalize();
    let ref_vec = if dir.y.abs() < 0.95 { Vec3::Y } else { Vec3::X };
    let right = dir.cross(ref_vec).normalize();
    let fwd = right.cross(dir);

    for ring in 0..2 {
        let center = if ring == 0 { start } else { end };
        let radius = if ring == 0 { start_r } else { end_r };
        for i in 0..CYL_SIDES {
            let a = (i as f32 / CYL_SIDES as f32) * std::f32::consts::TAU;
            let ca = a.cos();
            let sa = a.sin();
            let nx = right.x * ca + fwd.x * sa;
            let ny = right.y * ca + fwd.y * sa;
            let nz = right.z * ca + fwd.z * sa;
            verts.push(PlantVertex {
                position: [
                    center.x + nx * radius,
                    center.y + ny * radius,
                    center.z + nz * radius,
                ],
                normal: [nx, ny, nz],
                color,
            });
        }
    }

    let sides = CYL_SIDES as u32;
    for i in 0..sides {
        let i0 = base_idx + i;
        let i1 = base_idx + (i + 1) % sides;
        let i2 = base_idx + sides + i;
        let i3 = base_idx + sides + (i + 1) % sides;
        indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
    }
}
