use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;

use super::geometry::{append_box, append_octahedron, append_quad, append_triangle, Vertex};
#[cfg(not(target_arch = "wasm32"))]
use super::model_loader;
use crate::world_core::chunk::{FernInstance, HouseInstance, TreeInstance};

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

/// Registry of named prototype meshes. Tries to load GLB models from
/// `assets/models/`, falling back to procedural geometry when files are missing.
pub struct ModelRegistry {
    pub models: HashMap<String, PrototypeMesh>,
    /// True when the "tree" model is a single unified mesh (loaded from GLB)
    /// rather than the separate trunk/canopy procedural meshes.
    pub unified_tree: bool,
}

impl ModelRegistry {
    pub fn new(device: &wgpu::Device) -> Self {
        let mut models = HashMap::new();
        let mut unified_tree = false;

        // Try loading GLB models, fall back to procedural geometry
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some(mesh) = model_loader::try_load_model(device, "tree") {
                models.insert("tree".to_string(), mesh);
                unified_tree = true;
            }

            if let Some(mesh) = model_loader::try_load_model(device, "house") {
                models.insert("house".to_string(), mesh);
            }

            if let Some(mesh) = model_loader::try_load_model(device, "fern") {
                models.insert("fern".to_string(), mesh);
            }
        }

        // Suppress unused_mut on wasm32 where the cfg block above is compiled out
        let _ = &mut unified_tree;

        // Procedural fallbacks
        let mut verts = Vec::new();
        let mut idxs = Vec::new();

        if !models.contains_key("tree") {
            // Separate trunk and canopy meshes (legacy procedural)
            append_box(
                &mut verts,
                &mut idxs,
                Vec3::ZERO,
                Vec3::splat(0.5),
                Vec3::ONE,
            );
            models.insert(
                "trunk".to_string(),
                upload_prototype(device, &verts, &idxs, "unit-box"),
            );

            verts.clear();
            idxs.clear();
            append_octahedron(&mut verts, &mut idxs, Vec3::ZERO, 1.0, Vec3::ONE);
            models.insert(
                "canopy".to_string(),
                upload_prototype(device, &verts, &idxs, "unit-octahedron"),
            );
        }

        if !models.contains_key("house") {
            verts.clear();
            idxs.clear();
            build_house_prototype(&mut verts, &mut idxs);
            models.insert(
                "house".to_string(),
                upload_prototype(device, &verts, &idxs, "house-prototype"),
            );
        }

        if !models.contains_key("fern") {
            verts.clear();
            idxs.clear();
            build_fern_prototype(&mut verts, &mut idxs);
            models.insert(
                "fern".to_string(),
                upload_prototype(device, &verts, &idxs, "fern-prototype"),
            );
        }

        Self {
            models,
            unified_tree,
        }
    }

    pub fn get(&self, name: &str) -> Option<&PrototypeMesh> {
        self.models.get(name)
    }

    /// Replace a prototype mesh by name, dropping the old GPU buffers.
    /// Returns `true` if a procedural→GLB tree transition occurred
    /// (meaning tree instance buffers need to be rebuilt).
    pub fn hot_swap(&mut self, name: &str, mesh: PrototypeMesh) -> bool {
        self.models.insert(name.to_string(), mesh);

        let mut tree_transition = false;
        if name == "tree" && !self.unified_tree {
            self.unified_tree = true;
            self.models.remove("trunk");
            self.models.remove("canopy");
            tree_transition = true;
        }

        tree_transition
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

fn build_house_prototype(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    let wall_color = Vec3::new(0.72, 0.63, 0.46);
    let roof_color = Vec3::new(0.55, 0.22, 0.15);
    let half_w = 2.5;
    let half_d = 2.0;
    let wall_h = 3.0;
    let roof_h = 2.0;

    let bl = Vec3::new(-half_w, 0.0, -half_d);
    let br = Vec3::new(half_w, 0.0, -half_d);
    let fr = Vec3::new(half_w, 0.0, half_d);
    let fl = Vec3::new(-half_w, 0.0, half_d);
    let tbl = bl + Vec3::Y * wall_h;
    let tbr = br + Vec3::Y * wall_h;
    let tfr = fr + Vec3::Y * wall_h;
    let tfl = fl + Vec3::Y * wall_h;
    let ridge_l = Vec3::new(-half_w, wall_h + roof_h, 0.0);
    let ridge_r = Vec3::new(half_w, wall_h + roof_h, 0.0);

    // Walls
    append_quad(vertices, indices, [fl, fr, tfr, tfl], Vec3::Z, wall_color);
    append_quad(
        vertices,
        indices,
        [br, bl, tbl, tbr],
        Vec3::NEG_Z,
        wall_color,
    );
    append_quad(vertices, indices, [fr, br, tbr, tfr], Vec3::X, wall_color);
    append_quad(
        vertices,
        indices,
        [bl, fl, tfl, tbl],
        Vec3::NEG_X,
        wall_color,
    );

    // Roof slopes
    let roof_n_front = (Vec3::Z * half_d + Vec3::Y * roof_h).normalize();
    append_quad(
        vertices,
        indices,
        [tfl, tfr, ridge_r, ridge_l],
        roof_n_front,
        roof_color,
    );
    let roof_n_back = (Vec3::NEG_Z * half_d + Vec3::Y * roof_h).normalize();
    append_quad(
        vertices,
        indices,
        [tbr, tbl, ridge_l, ridge_r],
        roof_n_back,
        roof_color,
    );

    // Gable ends
    append_triangle(vertices, indices, tbl, tfl, ridge_l, roof_color);
    append_triangle(vertices, indices, tfr, tbr, ridge_r, roof_color);
}

/// Build instances for a unified tree model (single mesh with baked trunk + canopy).
/// Uses average height as uniform scale, positioned at the tree base.
pub fn build_tree_instances(trees: &[TreeInstance]) -> Vec<InstanceData> {
    trees
        .iter()
        .map(|t| {
            let total_height = t.trunk_height + t.canopy_radius * 2.0;
            let scale = total_height / 10.0; // normalize to ~10m reference height
            InstanceData {
                position: [t.position.x, t.position.y, t.position.z],
                rotation_y: 0.0,
                scale: [scale, scale, scale],
                _pad: 0.0,
                color: [1.0, 1.0, 1.0, 1.0], // colors baked in the model
            }
        })
        .collect()
}

pub fn build_trunk_instances(trees: &[TreeInstance]) -> Vec<InstanceData> {
    trees
        .iter()
        .map(|t| InstanceData {
            position: [
                t.position.x,
                t.position.y + t.trunk_height * 0.5,
                t.position.z,
            ],
            rotation_y: 0.0,
            scale: [0.30, t.trunk_height * 0.5, 0.30],
            _pad: 0.0,
            color: [0.33, 0.22, 0.11, 1.0],
        })
        .collect()
}

pub fn build_canopy_instances(trees: &[TreeInstance]) -> Vec<InstanceData> {
    trees
        .iter()
        .map(|t| InstanceData {
            position: [
                t.position.x,
                t.position.y + t.trunk_height + t.canopy_radius,
                t.position.z,
            ],
            rotation_y: 0.0,
            scale: [t.canopy_radius, t.canopy_radius, t.canopy_radius],
            _pad: 0.0,
            color: [0.14, 0.38, 0.16, 1.0],
        })
        .collect()
}

pub fn build_fern_instances(ferns: &[FernInstance]) -> Vec<InstanceData> {
    ferns
        .iter()
        .map(|f| InstanceData {
            position: [f.position.x, f.position.y, f.position.z],
            rotation_y: f.rotation,
            scale: [f.scale, f.scale, f.scale],
            _pad: 0.0,
            color: [1.0, 1.0, 1.0, 1.0],
        })
        .collect()
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

fn build_fern_prototype(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    let color = Vec3::new(0.20, 0.45, 0.15);
    let hw = 0.3; // half-width
    let h = 0.4; // height

    // Two crossed quads forming an X
    // Quad 1: along X axis
    append_quad(
        vertices,
        indices,
        [
            Vec3::new(-hw, 0.0, 0.0),
            Vec3::new(hw, 0.0, 0.0),
            Vec3::new(hw, h, 0.0),
            Vec3::new(-hw, h, 0.0),
        ],
        Vec3::Z,
        color,
    );

    // Quad 2: along Z axis
    append_quad(
        vertices,
        indices,
        [
            Vec3::new(0.0, 0.0, -hw),
            Vec3::new(0.0, 0.0, hw),
            Vec3::new(0.0, h, hw),
            Vec3::new(0.0, h, -hw),
        ],
        Vec3::X,
        color,
    );
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
