use std::collections::HashMap;

use glam::IVec2;

use super::geometry::Vertex;
use super::pipeline::create_render_pipeline;
use super::terrain_compute::{GpuTerrainChunk, TerrainComputePipeline};
use crate::world_core::chunk::{ChunkData, CHUNK_GRID_RESOLUTION};

pub struct TerrainPass {
    pipeline: wgpu::RenderPipeline,
    compute: TerrainComputePipeline,
    chunks: HashMap<IVec2, GpuTerrainChunk>,
}

impl TerrainPass {
    pub fn new(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        pipeline_layout: &wgpu::PipelineLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/terrain.wgsl").into()),
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

        let pipeline = create_render_pipeline(
            device,
            config,
            pipeline_layout,
            &shader,
            std::slice::from_ref(&vertex_layout),
            "terrain-pipeline",
        );

        Self {
            pipeline,
            compute: TerrainComputePipeline::new(device),
            chunks: HashMap::new(),
        }
    }

    /// Retains only chunks present in `world_chunks`, generates missing terrain.
    /// Returns `true` if any compute dispatches were recorded into `encoder`.
    pub fn sync_chunks(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        world_chunks: &HashMap<IVec2, ChunkData>,
    ) -> bool {
        self.chunks
            .retain(|coord, _| world_chunks.contains_key(coord));

        let mut dispatched = false;
        for (coord, chunk) in world_chunks {
            if self.chunks.contains_key(coord) {
                continue;
            }
            let total = CHUNK_GRID_RESOLUTION * CHUNK_GRID_RESOLUTION;
            if chunk.terrain.heights.len() == total
                && chunk.terrain.moisture.len() == total
                && chunk.terrain.max_height >= chunk.terrain.min_height
            {
                let gpu = self.compute.generate_chunk(
                    device,
                    encoder,
                    *coord,
                    &chunk.terrain.heights,
                    &chunk.terrain.moisture,
                );
                self.chunks.insert(*coord, gpu);
                dispatched = true;
            }
        }
        dispatched
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_index_buffer(
            self.compute.shared_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        for chunk in self.chunks.values() {
            pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
            pass.draw_indexed(0..self.compute.shared_index_count, 0, 0..1);
        }
    }
}
