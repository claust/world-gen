use std::collections::HashMap;

use glam::IVec2;

use super::geometry::Vertex;
use super::instancing::{
    build_canopy_instances, build_house_instances, build_tree_instances, build_trunk_instances,
    upload_instances, GpuInstanceChunk, InstanceData, ModelRegistry,
};
use super::pipeline::create_render_pipeline;
use crate::world_core::chunk::ChunkData;

pub struct InstancedPass {
    pipeline: wgpu::RenderPipeline,
    models: ModelRegistry,
    // Unified tree instances (used when a "tree" GLB model is loaded)
    tree_instances: HashMap<IVec2, GpuInstanceChunk>,
    // Legacy split instances (used with procedural trunk/canopy meshes)
    trunk_instances: HashMap<IVec2, GpuInstanceChunk>,
    canopy_instances: HashMap<IVec2, GpuInstanceChunk>,
    house_instances: HashMap<IVec2, GpuInstanceChunk>,
}

impl InstancedPass {
    pub fn new(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        pipeline_layout: &wgpu::PipelineLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/instanced.wgsl").into()),
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        };

        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceData>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
            ],
        };

        let pipeline = create_render_pipeline(
            device,
            config,
            pipeline_layout,
            &shader,
            &[vertex_layout, instance_layout],
            "instanced-pipeline",
        );

        Self {
            pipeline,
            models: ModelRegistry::new(device),
            tree_instances: HashMap::new(),
            trunk_instances: HashMap::new(),
            canopy_instances: HashMap::new(),
            house_instances: HashMap::new(),
        }
    }

    /// Retains only chunks present in `world_chunks`, builds missing instance buffers.
    pub fn sync_chunks(&mut self, device: &wgpu::Device, world_chunks: &HashMap<IVec2, ChunkData>) {
        self.tree_instances
            .retain(|coord, _| world_chunks.contains_key(coord));
        self.trunk_instances
            .retain(|coord, _| world_chunks.contains_key(coord));
        self.canopy_instances
            .retain(|coord, _| world_chunks.contains_key(coord));
        self.house_instances
            .retain(|coord, _| world_chunks.contains_key(coord));

        for (coord, chunk) in world_chunks {
            // Trees
            if self.models.unified_tree {
                if !self.tree_instances.contains_key(coord) {
                    if let Some(gpu) = upload_instances(
                        device,
                        &build_tree_instances(&chunk.content.trees),
                        "tree",
                    ) {
                        self.tree_instances.insert(*coord, gpu);
                    }
                }
            } else {
                if !self.trunk_instances.contains_key(coord) {
                    if let Some(gpu) = upload_instances(
                        device,
                        &build_trunk_instances(&chunk.content.trees),
                        "trunk",
                    ) {
                        self.trunk_instances.insert(*coord, gpu);
                    }
                }
                if !self.canopy_instances.contains_key(coord) {
                    if let Some(gpu) = upload_instances(
                        device,
                        &build_canopy_instances(&chunk.content.trees),
                        "canopy",
                    ) {
                        self.canopy_instances.insert(*coord, gpu);
                    }
                }
            }

            // Houses
            if !self.house_instances.contains_key(coord) {
                if let Some(gpu) = upload_instances(
                    device,
                    &build_house_instances(&chunk.content.houses),
                    "house",
                ) {
                    self.house_instances.insert(*coord, gpu);
                }
            }
        }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);

        // Trees
        if self.models.unified_tree {
            if let Some(tree_mesh) = self.models.get("tree") {
                pass.set_vertex_buffer(0, tree_mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(tree_mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for inst in self.tree_instances.values() {
                    pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                    pass.draw_indexed(0..tree_mesh.index_count, 0, 0..inst.instance_count);
                }
            }
        } else {
            // Legacy: separate trunk + canopy
            if let Some(trunk_mesh) = self.models.get("trunk") {
                pass.set_vertex_buffer(0, trunk_mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(trunk_mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for inst in self.trunk_instances.values() {
                    pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                    pass.draw_indexed(0..trunk_mesh.index_count, 0, 0..inst.instance_count);
                }
            }

            if let Some(canopy_mesh) = self.models.get("canopy") {
                pass.set_vertex_buffer(0, canopy_mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    canopy_mesh.index_buffer.slice(..),
                    wgpu::IndexFormat::Uint32,
                );
                for inst in self.canopy_instances.values() {
                    pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                    pass.draw_indexed(0..canopy_mesh.index_count, 0, 0..inst.instance_count);
                }
            }
        }

        // Houses
        if let Some(house_mesh) = self.models.get("house") {
            pass.set_vertex_buffer(0, house_mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(house_mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for inst in self.house_instances.values() {
                pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                pass.draw_indexed(0..house_mesh.index_count, 0, 0..inst.instance_count);
            }
        }
    }
}
