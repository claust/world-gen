use std::collections::HashMap;

use glam::IVec2;

use super::geometry::Vertex;
use super::instancing::{
    build_fern_instances, build_house_instances, build_tree_instances, upload_instances,
    GpuInstanceChunk, InstanceData, ModelRegistry,
};
#[cfg(not(target_arch = "wasm32"))]
use super::model_loader;
use super::pipeline::create_render_pipeline;
use crate::world_core::chunk::ChunkData;

pub struct InstancedPass {
    pipeline: wgpu::RenderPipeline,
    models: ModelRegistry,
    tree_instances: HashMap<IVec2, GpuInstanceChunk>,
    house_instances: HashMap<IVec2, GpuInstanceChunk>,
    fern_instances: HashMap<IVec2, GpuInstanceChunk>,
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
            house_instances: HashMap::new(),
            fern_instances: HashMap::new(),
        }
    }

    /// Retains only chunks present in `world_chunks`, builds missing instance buffers.
    pub fn sync_chunks(&mut self, device: &wgpu::Device, world_chunks: &HashMap<IVec2, ChunkData>) {
        self.tree_instances
            .retain(|coord, _| world_chunks.contains_key(coord));
        self.house_instances
            .retain(|coord, _| world_chunks.contains_key(coord));
        self.fern_instances
            .retain(|coord, _| world_chunks.contains_key(coord));

        for (coord, chunk) in world_chunks {
            // Trees
            if !self.tree_instances.contains_key(coord) {
                if let Some(gpu) =
                    upload_instances(device, &build_tree_instances(&chunk.content.trees), "tree")
                {
                    self.tree_instances.insert(*coord, gpu);
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

            // Ferns
            if !self.fern_instances.contains_key(coord) {
                if let Some(gpu) =
                    upload_instances(device, &build_fern_instances(&chunk.content.ferns), "fern")
                {
                    self.fern_instances.insert(*coord, gpu);
                }
            }
        }
    }

    /// Process hot-reloaded model data. Parses GLB bytes, uploads to GPU, and
    /// swaps the prototype mesh. On a procedural→GLB tree transition, clears
    /// instance buffers so `sync_chunks()` rebuilds them.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn apply_model_reloads(&mut self, device: &wgpu::Device, reloads: &[(String, Vec<u8>)]) {
        for (name, bytes) in reloads {
            match model_loader::load_glb(device, bytes, name) {
                Ok(mesh) => {
                    self.models.hot_swap(name, mesh);
                    log::info!("hot-reloaded model: {name}");

                    match name.as_str() {
                        "tree" => self.tree_instances.clear(),
                        "house" => self.house_instances.clear(),
                        "fern" => self.fern_instances.clear(),
                        _ => {}
                    }
                }
                Err(e) => {
                    log::warn!("failed to hot-reload model {name}: {e:#}");
                }
            }
        }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);

        let draw_model =
            |pass: &mut wgpu::RenderPass<'a>,
             name: &str,
             instances: &'a HashMap<IVec2, GpuInstanceChunk>| {
                if let Some(mesh) = self.models.get(name) {
                    pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                    pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    for inst in instances.values() {
                        pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                        pass.draw_indexed(0..mesh.index_count, 0, 0..inst.instance_count);
                    }
                }
            };

        draw_model(pass, "tree", &self.tree_instances);
        draw_model(pass, "house", &self.house_instances);
        draw_model(pass, "fern", &self.fern_instances);
    }
}
