use std::collections::HashMap;

use glam::IVec2;
use wgpu::util::DeviceExt;

use super::geometry::Vertex;
use super::pipeline::create_water_pipeline;
use crate::world_core::chunk::{ChunkData, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS};

struct GpuWaterChunk {
    vertex_buffer: wgpu::Buffer,
}

pub struct WaterPass {
    pipeline: wgpu::RenderPipeline,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    chunks: HashMap<IVec2, GpuWaterChunk>,
    sea_level: f32,
}

impl WaterPass {
    pub fn new(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        pipeline_layout: &wgpu::PipelineLayout,
        sea_level: f32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/water.wgsl").into()),
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

        let pipeline = create_water_pipeline(
            device,
            config,
            pipeline_layout,
            &shader,
            std::slice::from_ref(&vertex_layout),
            "water-pipeline",
        );

        // Shared index buffer — same grid topology as terrain (128×128 quads).
        let side = CHUNK_GRID_RESOLUTION;
        let mut indices = Vec::with_capacity((side - 1) * (side - 1) * 6);
        for z in 0..(side - 1) {
            for x in 0..(side - 1) {
                let i0 = (z * side + x) as u32;
                let i1 = i0 + 1;
                let i2 = i0 + side as u32;
                let i3 = i2 + 1;
                indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
            }
        }

        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water-shared-ib"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            pipeline,
            index_buffer,
            index_count: indices.len() as u32,
            chunks: HashMap::new(),
            sea_level,
        }
    }

    /// Retains only chunks present in `world_chunks`. Generates water mesh for
    /// any new chunk whose terrain dips below sea level.
    pub fn sync_chunks(&mut self, device: &wgpu::Device, world_chunks: &HashMap<IVec2, ChunkData>) {
        self.chunks
            .retain(|coord, _| world_chunks.contains_key(coord));

        let side = CHUNK_GRID_RESOLUTION;
        let total = side * side;
        let cell_size = CHUNK_SIZE_METERS / (side - 1) as f32;

        for (coord, chunk) in world_chunks {
            if self.chunks.contains_key(coord) {
                continue;
            }
            if !chunk.terrain.has_water {
                continue;
            }

            let origin_x = coord.x as f32 * CHUNK_SIZE_METERS;
            let origin_z = coord.y as f32 * CHUNK_SIZE_METERS;

            let up = [0.0_f32, 1.0, 0.0];
            let color = [0.12_f32, 0.30, 0.45];

            let mut vertices = Vec::with_capacity(total);
            for z in 0..side {
                for x in 0..side {
                    let wx = origin_x + x as f32 * cell_size;
                    let wz = origin_z + z as f32 * cell_size;
                    vertices.push(Vertex {
                        position: [wx, self.sea_level, wz],
                        normal: up,
                        color,
                    });
                }
            }

            let label = format!("water-vb-{},{}", coord.x, coord.y);
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&label),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            self.chunks.insert(*coord, GpuWaterChunk { vertex_buffer });
        }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.chunks.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for chunk in self.chunks.values() {
            pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
    }
}
