use bytemuck::{Pod, Zeroable};
use glam::IVec2;
use wgpu::util::DeviceExt;

use crate::world_core::chunk::{CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS};

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct ChunkParams {
    origin_x: f32,
    origin_z: f32,
    cell_size: f32,
    side: u32,
}

pub struct GpuTerrainChunk {
    pub vertex_buffer: wgpu::Buffer,
}

pub struct TerrainComputePipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    pub shared_index_buffer: wgpu::Buffer,
    pub shared_index_count: u32,
}

impl TerrainComputePipeline {
    pub fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain-gen-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/terrain_gen.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain-gen-bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain-gen-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("terrain-gen-pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

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

        let shared_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain-shared-ib"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        Self {
            pipeline,
            bind_group_layout,
            shared_index_buffer,
            shared_index_count: indices.len() as u32,
        }
    }

    pub fn generate_chunk(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        coord: IVec2,
        heights: &[f32],
        moisture: &[f32],
    ) -> GpuTerrainChunk {
        let side = CHUNK_GRID_RESOLUTION;
        let total = side * side;
        let cell_size = CHUNK_SIZE_METERS / (side - 1) as f32;

        let params = ChunkParams {
            origin_x: coord.x as f32 * CHUNK_SIZE_METERS,
            origin_z: coord.y as f32 * CHUNK_SIZE_METERS,
            cell_size,
            side: side as u32,
        };

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain-gen-params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let heights_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain-gen-heights"),
            contents: bytemuck::cast_slice(heights),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let moisture_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain-gen-moisture"),
            contents: bytemuck::cast_slice(moisture),
            usage: wgpu::BufferUsages::STORAGE,
        });

        // 9 floats per vertex (position, normal, color) × 4 bytes
        let output_size = (total * 9 * 4) as u64;
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain-gen-output"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain-gen-bg"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: heights_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: moisture_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: vertex_buffer.as_entire_binding(),
                },
            ],
        });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("terrain-gen-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let wg = (side as u32).div_ceil(16);
            pass.dispatch_workgroups(wg, wg, 1);
        }

        GpuTerrainChunk { vertex_buffer }
    }
}
