// River water surface pass: see shaders/river.wgsl for the animated surface.
use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::IVec2;
use wgpu::util::DeviceExt;

use super::frustum::Frustum;
use super::pipeline::create_water_pipeline;
use crate::world_core::chunk::{
    ChunkData, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS, RIVER_SURFACE_THRESHOLD,
};

/// Lift of the river water surface above the carved bed, in metres. The sheet
/// hugs the bed at the banks (low wetness) and rises toward the channel centre
/// (high wetness), so deeper channels fill more and the surface self-levels.
const RIVER_MIN_DEPTH: f32 = 0.35;
const RIVER_DEPTH_SCALE: f32 = 6.0;

/// One vertex of the river water surface. Carries the downstream flow direction
/// and wetness so the shader can animate the streaming ripples and fade the
/// shallow banks. Position y is the water surface height (bed + depth).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RiverVertex {
    position: [f32; 3],
    flow: [f32; 2],
    wetness: f32,
}

struct GpuRiverChunk {
    vertex_buffer: wgpu::Buffer,
}

pub struct RiverPass {
    pipeline: wgpu::RenderPipeline,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    chunks: HashMap<IVec2, GpuRiverChunk>,
}

impl RiverPass {
    pub fn new(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        frame_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        shadow_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river-shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shaders/lighting.wgsl"),
                    include_str!("shaders/river.wgsl")
                )
                .into(),
            ),
        });

        // Rivers sample the shadow map at group 2, like the sea water pass.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("river-pipeline-layout"),
            bind_group_layouts: &[frame_layout, material_layout, shadow_layout],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<RiverVertex>() as u64,
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
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: 20,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32,
                },
            ],
        };

        let pipeline = create_water_pipeline(
            device,
            render_format,
            &pipeline_layout,
            &shader,
            std::slice::from_ref(&vertex_layout),
            "river-pipeline",
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
            label: Some("river-shared-ib"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            pipeline,
            index_buffer,
            index_count: indices.len() as u32,
            chunks: HashMap::new(),
        }
    }

    /// Retains only chunks present in `world_chunks`. Builds a river water mesh
    /// for any new chunk whose terrain carries flowing water. The mesh reuses the
    /// full terrain grid; the shader discards the dry fringe per-fragment.
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
            if !chunk.terrain.has_river {
                continue;
            }

            let origin_x = coord.x as f32 * CHUNK_SIZE_METERS;
            let origin_z = coord.y as f32 * CHUNK_SIZE_METERS;
            let terrain = &chunk.terrain;

            let mut vertices = Vec::with_capacity(total);
            for z in 0..side {
                for x in 0..side {
                    let idx = z * side + x;
                    let wx = origin_x + x as f32 * cell_size;
                    let wz = origin_z + z as f32 * cell_size;
                    let wet = terrain.river[idx];
                    // Lift the surface above the carved bed only where there is
                    // actually water; dry vertices stay at the bed and are
                    // discarded by the shader.
                    let depth = if wet > RIVER_SURFACE_THRESHOLD {
                        RIVER_MIN_DEPTH + wet * RIVER_DEPTH_SCALE
                    } else {
                        0.0
                    };
                    vertices.push(RiverVertex {
                        position: [wx, terrain.heights[idx] + depth, wz],
                        flow: terrain.river_flow[idx],
                        wetness: wet,
                    });
                }
            }

            let label = format!("river-vb-{},{}", coord.x, coord.y);
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&label),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });

            self.chunks.insert(*coord, GpuRiverChunk { vertex_buffer });
        }
    }

    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frustum: &Frustum,
        shadow_bind_group: &'a wgpu::BindGroup,
    ) {
        if self.chunks.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(2, shadow_bind_group, &[]);
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        for (coord, chunk) in &self.chunks {
            if !frustum.is_chunk_visible(*coord) {
                continue;
            }
            pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
    }
}
