use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::geometry::Vertex;
use super::model_loader;
use crate::world_core::chunk::{HouseInstance, PlantInstance};
use crate::world_core::herbarium::PlantSpeciesInfo;

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

/// Build per-species instance data from plant instances.
/// Returns a Vec where index i = instances for species i.
pub fn build_plant_instances(
    plants: &[PlantInstance],
    species: &[PlantSpeciesInfo],
) -> Vec<Vec<InstanceData>> {
    let mut per_species: Vec<Vec<InstanceData>> = vec![Vec::new(); species.len()];

    for p in plants {
        let idx = p.species_index;
        if idx >= species.len() {
            continue;
        }
        let ref_height = (species[idx].height_range[0] + species[idx].height_range[1]) * 0.5;
        let scale = p.height / ref_height.max(0.01);

        per_species[idx].push(InstanceData {
            position: [p.position.x, p.position.y, p.position.z],
            rotation_y: p.rotation,
            scale: [scale, scale, scale],
            _pad: 0.0,
            color: [1.0, 1.0, 1.0, 1.0], // colors baked in procedural mesh
        });
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
