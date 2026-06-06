use std::collections::HashMap;

use glam::IVec2;

use super::frustum::Frustum;
use super::pipeline::{create_render_pipeline, create_shadow_pipeline};
use super::terrain_compute::{GpuTerrainChunk, TerrainComputePipeline, TERRAIN_VERTEX_FLOATS};
use crate::world_core::chunk::{ChunkData, CHUNK_GRID_RESOLUTION};

pub struct TerrainPass {
    pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    compute: TerrainComputePipeline,
    chunks: HashMap<IVec2, GpuTerrainChunk>,
}

impl TerrainPass {
    pub fn new(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        frame_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
        shadow_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain-shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shaders/lighting.wgsl"),
                    include_str!("shaders/terrain.wgsl")
                )
                .into(),
            ),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain-pipeline-layout"),
            bind_group_layouts: &[frame_layout, material_layout, texture_layout, shadow_layout],
            push_constant_ranges: &[],
        });

        // Matches the compute shader's per-vertex layout: position(3),
        // normal(3), biome_data(3), river wetness(1) = TERRAIN_VERTEX_FLOATS.
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: (TERRAIN_VERTEX_FLOATS * 4) as u64,
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
                wgpu::VertexAttribute {
                    offset: 36,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32,
                },
            ],
        };

        let pipeline = create_render_pipeline(
            device,
            render_format,
            &pipeline_layout,
            &shader,
            std::slice::from_ref(&vertex_layout),
            "terrain-pipeline",
        );

        // Depth-only shadow pipeline (group 0 / frame only).
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain-shadow-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shadows.wgsl").into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("terrain-shadow-pipeline-layout"),
                bind_group_layouts: &[frame_layout],
                push_constant_ranges: &[],
            });
        let shadow_pipeline = create_shadow_pipeline(
            device,
            &shadow_pipeline_layout,
            &shadow_shader,
            "vs_terrain",
            std::slice::from_ref(&vertex_layout),
            "terrain-shadow-pipeline",
        );

        Self {
            pipeline,
            shadow_pipeline,
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
                && chunk.terrain.river.len() == total
                && chunk.terrain.max_height >= chunk.terrain.min_height
            {
                let gpu = self.compute.generate_chunk(
                    device,
                    encoder,
                    *coord,
                    &chunk.terrain.heights,
                    &chunk.terrain.moisture,
                    &chunk.terrain.river,
                );
                self.chunks.insert(*coord, gpu);
                dispatched = true;
            }
        }
        dispatched
    }

    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frustum: &Frustum,
        texture_bind_group: &'a wgpu::BindGroup,
        shadow_bind_group: &'a wgpu::BindGroup,
    ) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(2, texture_bind_group, &[]);
        pass.set_bind_group(3, shadow_bind_group, &[]);
        pass.set_index_buffer(
            self.compute.shared_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        for (coord, chunk) in &self.chunks {
            if !frustum.is_chunk_visible(*coord) {
                continue;
            }
            pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
            pass.draw_indexed(0..self.compute.shared_index_count, 0, 0..1);
        }
    }

    /// Depth-only pass: re-draw every loaded chunk from the light's point of
    /// view. No frustum culling so off-screen terrain still casts shadows.
    /// Assumes the caller has bound group 0 (frame uniforms).
    pub fn render_depth<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.shadow_pipeline);
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
