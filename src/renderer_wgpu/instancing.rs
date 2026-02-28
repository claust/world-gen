use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::geometry::Vertex;
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

/// Registry of named prototype meshes loaded from GLB models in `assets/models/`.
pub struct ModelRegistry {
    pub models: HashMap<String, PrototypeMesh>,
}

impl ModelRegistry {
    pub fn new(device: &wgpu::Device) -> Self {
        let mut models = HashMap::new();

        #[cfg(not(target_arch = "wasm32"))]
        for name in ["tree", "house", "fern"] {
            if let Some(mesh) = model_loader::try_load_model(device, name) {
                models.insert(name.to_string(), mesh);
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

/// Build instances for a tree model (single mesh with baked trunk + canopy).
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
