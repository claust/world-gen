use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::geometry::Vertex;
use super::model_loader;
use crate::world_core::chunk::{HouseInstance, PlantInstance};
use crate::world_core::herbarium::PlantSpeciesInfo;
use crate::world_core::lifecycle::GrowthStage;

#[repr(C)]
#[derive(Clone, Copy, Debug, Zeroable, Pod)]
pub struct InstanceData {
    pub position: [f32; 3],
    pub rotation_y: f32,
    pub scale: [f32; 3],
    pub _pad: f32,
    pub color: [f32; 4],
}

pub struct PrototypeMesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

pub struct GpuInstanceChunk {
    pub instance_buffer: wgpu::Buffer,
    pub instance_count: u32,
}

/// Registry of named prototype meshes.
pub struct ModelRegistry {
    pub models: HashMap<String, PrototypeMesh>,
}

impl ModelRegistry {
    pub fn new(device: &wgpu::Device) -> Self {
        let mut models = HashMap::new();

        // Only load the house model from GLB; plant meshes are generated procedurally
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(mesh) = model_loader::try_load_model(device, "house") {
                models.insert("house".to_string(), mesh);
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            let embedded: &[(&str, &[u8])] =
                &[("house", include_bytes!("../../assets/models/house.glb"))];
            for (name, bytes) in embedded {
                match model_loader::load_glb(device, bytes, name) {
                    Ok(mesh) => {
                        log::info!("Loaded embedded model: {name}");
                        models.insert(name.to_string(), mesh);
                    }
                    Err(e) => log::warn!("Failed to load embedded model {name}: {e:#}"),
                }
            }
        }

        Self { models }
    }

    pub fn get(&self, name: &str) -> Option<&PrototypeMesh> {
        self.models.get(name)
    }

    /// Replace a prototype mesh by name, dropping the old GPU buffers.
    pub fn hot_swap(&mut self, name: &str, mesh: PrototypeMesh) {
        self.models.insert(name.to_string(), mesh);
    }
}

pub fn upload_prototype(
    device: &wgpu::Device,
    vertices: &[Vertex],
    indices: &[u32],
    label: &str,
) -> PrototypeMesh {
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{label}-vb")),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{label}-ib")),
        contents: bytemuck::cast_slice(indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    PrototypeMesh {
        vertex_buffer,
        index_buffer,
        index_count: indices.len() as u32,
    }
}

/// Unit-ish crossed-quad billboard for shrubs: two vertical quads at 90° plus a
/// horizontal top cap, sized to `height` (natural world height) and `half_width`.
///
/// - Normals are up-biased outward (as if the shrub were a hemisphere) so the
///   day/night lighting rounds the silhouette instead of reading as flat cards.
/// - The per-vertex UV is packed into the colour attribute's xy channel; the
///   billboard shader reads it there to build the procedural blob mask.
///
/// ~12 verts / 6 tris replaces the 3.5–5.3K-vert procedural shrub mesh.
pub fn shrub_billboard_mesh(height: f32, half_width: f32) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts: Vec<Vertex> = Vec::with_capacity(12);
    let mut indices: Vec<u32> = Vec::with_capacity(18);
    let center = [0.0, height * 0.5, 0.0];
    let up_bias = 0.55 * height;

    let r = half_width;
    let h = height;
    let yc = h * 0.68; // top-cap height — biased up so it reads as a crown from above

    // Two vertical quads (XY plane and ZY plane) crossed at 90°, plus a
    // horizontal cap for top-down readability.
    let quads: [[[f32; 3]; 4]; 3] = [
        [[-r, 0.0, 0.0], [r, 0.0, 0.0], [r, h, 0.0], [-r, h, 0.0]],
        [[0.0, 0.0, -r], [0.0, 0.0, r], [0.0, h, r], [0.0, h, -r]],
        [[-r, yc, -r], [r, yc, -r], [r, yc, r], [-r, yc, r]],
    ];

    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    for corners in &quads {
        let base = verts.len() as u32;
        for (corner, uv) in corners.iter().zip(uvs.iter()) {
            let mut n = [
                corner[0] - center[0],
                corner[1] - center[1] + up_bias,
                corner[2] - center[2],
            ];
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-5);
            n = [n[0] / len, n[1] / len, n[2] / len];
            verts.push(Vertex {
                position: *corner,
                normal: n,
                color: [uv[0], uv[1], 0.0],
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    (verts, indices)
}

// --- Debug tile-marker road-post sign --------------------------------------
//
// A small post standing in the middle of each chunk with a flat board near the
// top. The chunk coordinate is painted onto the board by `SignTextPass`; these
// constants are shared so the text planes line up with the board faces.

/// Pole half-thickness in X and Z (meters).
pub const SIGN_POLE_HALF: f32 = 0.06;
/// Board center height above the ground (meters).
pub const SIGN_BOARD_CENTER_Y: f32 = 2.0;
/// Board half-width in X (meters).
pub const SIGN_BOARD_HALF_W: f32 = 0.80;
/// Board half-height in Y (meters).
pub const SIGN_BOARD_HALF_H: f32 = 0.34;
/// Board half-thickness in Z (meters). The board is opaque so each face
/// occludes the text painted on the opposite side.
pub const SIGN_BOARD_HALF_T: f32 = 0.05;

/// Append an axis-aligned box centered at `center` with half-extents `half`,
/// flat-shaded with `color`. Faces wind CCW as seen from outside (matches the
/// instanced pipeline's back-face culling).
fn push_box(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    center: [f32; 3],
    half: [f32; 3],
    color: [f32; 3],
) {
    // (normal, in-plane right axis u, in-plane up axis v) with u × v = normal.
    let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
        ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]), // +X
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), // -X
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]), // +Y
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]), // -Y
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),  // +Z
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), // -Z
    ];

    for (n, u, v) in faces {
        // Half-extent along each in-plane axis (pick the matching component).
        let hu = u[0].abs() * half[0] + u[1].abs() * half[1] + u[2].abs() * half[2];
        let hv = v[0].abs() * half[0] + v[1].abs() * half[1] + v[2].abs() * half[2];
        let hn = n[0].abs() * half[0] + n[1].abs() * half[1] + n[2].abs() * half[2];
        let fc = [
            center[0] + n[0] * hn,
            center[1] + n[1] * hn,
            center[2] + n[2] * hn,
        ];
        // Corners: bottom-left, bottom-right, top-right, top-left (CCW from outside).
        let corner = |su: f32, sv: f32| {
            [
                fc[0] + u[0] * hu * su + v[0] * hv * sv,
                fc[1] + u[1] * hu * su + v[1] * hv * sv,
                fc[2] + u[2] * hu * su + v[2] * hv * sv,
            ]
        };
        let base = verts.len() as u32;
        for pos in [
            corner(-1.0, -1.0),
            corner(1.0, -1.0),
            corner(1.0, 1.0),
            corner(-1.0, 1.0),
        ] {
            verts.push(Vertex {
                position: pos,
                normal: n,
                color,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Mesh for a single road-post sign: a wooden pole topped by a flat board.
/// Origin at ground level, centered on X/Z; the board faces ±Z.
pub fn sign_post_mesh() -> (Vec<Vertex>, Vec<u32>) {
    let mut verts = Vec::new();
    let mut indices = Vec::new();

    let pole_color = [0.40, 0.27, 0.15];
    let board_color = [0.88, 0.82, 0.66];

    // Pole: from the ground up to the board center.
    let pole_top = SIGN_BOARD_CENTER_Y;
    push_box(
        &mut verts,
        &mut indices,
        [0.0, pole_top * 0.5, 0.0],
        [SIGN_POLE_HALF, pole_top * 0.5, SIGN_POLE_HALF],
        pole_color,
    );

    // Board: centered at SIGN_BOARD_CENTER_Y, facing ±Z.
    push_box(
        &mut verts,
        &mut indices,
        [0.0, SIGN_BOARD_CENTER_Y, 0.0],
        [SIGN_BOARD_HALF_W, SIGN_BOARD_HALF_H, SIGN_BOARD_HALF_T],
        board_color,
    );

    (verts, indices)
}

/// Build per-species instance data from plant instances.
/// Returns a Vec where index i = (mature instances, forced-LOD instances) for species i.
pub fn build_plant_instances(
    plants: &[PlantInstance],
    species: &[PlantSpeciesInfo],
) -> Vec<(Vec<InstanceData>, Vec<InstanceData>)> {
    let mut per_species: Vec<(Vec<InstanceData>, Vec<InstanceData>)> = (0..species.len())
        .map(|_| (Vec::new(), Vec::new()))
        .collect();

    for p in plants {
        let idx = p.species_index;
        if idx >= species.len() {
            continue;
        }
        let ref_height = (species[idx].height_range[0] + species[idx].height_range[1]) * 0.5;
        let scale = (p.height / ref_height.max(0.01)) * p.growth_stage.scale_factor();

        // Procedural meshes bake their colours in (white tint). Billboards are a
        // flat untextured card, so they carry the species leaf colour as a tint.
        let color = if species[idx].kind == "shrub" {
            let c = species[idx].leaf_color;
            [c[0], c[1], c[2], 1.0]
        } else {
            [1.0, 1.0, 1.0, 1.0]
        };

        let instance = InstanceData {
            position: [p.position.x, p.position.y, p.position.z],
            rotation_y: p.rotation,
            scale: [scale, scale, scale],
            _pad: 0.0,
            color,
        };

        match p.growth_stage {
            GrowthStage::Mature => per_species[idx].0.push(instance),
            GrowthStage::Seedling | GrowthStage::Young => per_species[idx].1.push(instance),
        }
    }

    per_species
}

pub fn build_house_instances(houses: &[HouseInstance]) -> Vec<InstanceData> {
    houses
        .iter()
        .map(|h| InstanceData {
            position: [h.position.x, h.position.y, h.position.z],
            rotation_y: h.rotation,
            scale: [1.0, 1.0, 1.0],
            _pad: 0.0,
            color: [1.0, 1.0, 1.0, 1.0],
        })
        .collect()
}

pub fn upload_instances(
    device: &wgpu::Device,
    instances: &[InstanceData],
    label: &str,
) -> Option<GpuInstanceChunk> {
    if instances.is_empty() {
        return None;
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{label}-instance-buf")),
        contents: bytemuck::cast_slice(instances),
        usage: wgpu::BufferUsages::VERTEX,
    });
    Some(GpuInstanceChunk {
        instance_buffer: buffer,
        instance_count: instances.len() as u32,
    })
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::build_plant_instances;
    use crate::world_core::chunk::PlantInstance;
    use crate::world_core::herbarium::{Herbarium, PlantRegistry};
    use crate::world_core::lifecycle::GrowthStage;

    #[test]
    fn build_plant_instances_scales_from_growth_stage_and_splits_lod_groups() {
        let registry = PlantRegistry::from_herbarium(&Herbarium::default_seeded());
        let plants = vec![
            PlantInstance {
                position: Vec3::new(0.0, 0.0, 0.0),
                rotation: 0.0,
                height: 10.0,
                species_index: 0,
                growth_stage: GrowthStage::Mature,
            },
            PlantInstance {
                position: Vec3::new(1.0, 0.0, 0.0),
                rotation: 0.0,
                height: 10.0,
                species_index: 0,
                growth_stage: GrowthStage::Seedling,
            },
            PlantInstance {
                position: Vec3::new(2.0, 0.0, 0.0),
                rotation: 0.0,
                height: 10.0,
                species_index: 0,
                growth_stage: GrowthStage::Young,
            },
        ];

        let per_species = build_plant_instances(&plants, &registry.species);

        let ref_height =
            (registry.species[0].height_range[0] + registry.species[0].height_range[1]) * 0.5;
        let mature_scale = 10.0 / ref_height.max(0.01);

        assert_eq!(per_species[0].0.len(), 1);
        assert_eq!(per_species[0].1.len(), 2);
        assert!((per_species[0].0[0].scale[0] - mature_scale).abs() < 1e-5);
        assert!((per_species[0].1[0].scale[0] - mature_scale * 0.15).abs() < 1e-5);
        assert!((per_species[0].1[1].scale[0] - mature_scale * 0.5).abs() < 1e-5);
    }
}
