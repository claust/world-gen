use glam::Vec3;

use super::config::SpeciesConfig;
use super::tree::TreeData;
use super::PlantVertex;
use crate::world_core::color::hsl_to_linear;

const CYL_SIDES: usize = 8;

const PHI: f32 = 1.618_034;

#[rustfmt::skip]
const ICO_V: [[f32; 3]; 12] = [
    [-1.0,  PHI,  0.0],
    [ 1.0,  PHI,  0.0],
    [-1.0, -PHI,  0.0],
    [ 1.0, -PHI,  0.0],
    [ 0.0, -1.0,  PHI],
    [ 0.0,  1.0,  PHI],
    [ 0.0, -1.0, -PHI],
    [ 0.0,  1.0, -PHI],
    [ PHI,  0.0, -1.0],
    [ PHI,  0.0,  1.0],
    [-PHI,  0.0, -1.0],
    [-PHI,  0.0,  1.0],
];

#[rustfmt::skip]
const ICO_F: [[usize; 3]; 20] = [
    [0, 11, 5],  [0, 5, 1],   [0, 1, 7],   [0, 7, 10],  [0, 10, 11],
    [1, 5, 9],   [5, 11, 4],  [11, 10, 2], [10, 7, 6],  [7, 1, 8],
    [3, 9, 4],   [3, 4, 2],   [3, 2, 6],   [3, 6, 8],   [3, 8, 9],
    [4, 9, 5],   [2, 4, 11],  [6, 2, 10],  [8, 6, 7],   [9, 8, 1],
];

/// Precomputed normalized icosahedron vertices.
fn ico_normals() -> [[f32; 3]; 12] {
    let mut out = [[0.0; 3]; 12];
    for (i, v) in ICO_V.iter().enumerate() {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        out[i] = [v[0] / len, v[1] / len, v[2] / len];
    }
    out
}

pub fn build_mesh(spec: &SpeciesConfig, data: &TreeData) -> (Vec<PlantVertex>, Vec<u32>) {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    // Bark color (linear RGB)
    let bark_linear = hsl_to_linear(spec.color.bark.h, spec.color.bark.s, spec.color.bark.l);

    for seg in &data.segments {
        let depth_darken = 1.0 - seg.depth as f32 * 0.05;
        let color = [
            bark_linear[0] * depth_darken,
            bark_linear[1] * depth_darken,
            bark_linear[2] * depth_darken,
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

    // Leaf colors
    let leaf = &spec.color.leaf;
    let ico_n = ico_normals();

    for blob in &data.foliage {
        let h = leaf.h + blob.hue_shift;
        let l = (leaf.l + blob.light_shift).clamp(0.15, 0.6);
        let color = hsl_to_linear(h, leaf.s, l);
        add_icosahedron(
            blob.center,
            blob.radius,
            color,
            &ico_n,
            &mut vertices,
            &mut indices,
        );
    }

    (vertices, indices)
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

fn add_icosahedron(
    center: Vec3,
    radius: f32,
    color: [f32; 3],
    ico_n: &[[f32; 3]; 12],
    verts: &mut Vec<PlantVertex>,
    indices: &mut Vec<u32>,
) {
    let base_idx = verts.len() as u32;
    for (i, n) in ico_n.iter().enumerate() {
        let v = ICO_V[i];
        verts.push(PlantVertex {
            position: [
                center.x + n[0] * radius,
                center.y + n[1] * radius,
                center.z + n[2] * radius,
            ],
            normal: [v[0], v[1], v[2]],
            color,
        });
    }
    for f in &ICO_F {
        indices.extend_from_slice(&[
            base_idx + f[0] as u32,
            base_idx + f[1] as u32,
            base_idx + f[2] as u32,
        ]);
    }
}
