use std::collections::HashMap;

use glam::{IVec2, Mat4, Vec3};
use wgpu::util::DeviceExt;

use super::geometry::Vertex;
use super::material::{FrameBindGroup, FrameUniform, MaterialBindGroup};
use super::mesh::{build_house_mesh, build_terrain_mesh, build_tree_mesh, CpuChunkMesh};
use crate::renderer_wgpu::pipeline::DepthTexture;
use crate::world_core::chunk::ChunkData;

struct GpuChunk {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

pub struct WorldRenderer {
    frame_bg: FrameBindGroup,
    terrain_material: MaterialBindGroup,
    terrain_pipeline: wgpu::RenderPipeline,
    depth: DepthTexture,
    terrain_chunk_meshes: HashMap<IVec2, GpuChunk>,
    tree_chunk_meshes: HashMap<IVec2, GpuChunk>,
    house_chunk_meshes: HashMap<IVec2, GpuChunk>,
}

impl WorldRenderer {
    pub fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Self {
        let frame_bg = FrameBindGroup::new(device);
        let terrain_material = MaterialBindGroup::new_terrain(device);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/terrain.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain-pipeline-layout"),
            bind_group_layouts: &[&frame_bg.layout, &terrain_material.layout],
            push_constant_ranges: &[],
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

        let terrain_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[vertex_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::renderer_wgpu::pipeline::DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        Self {
            frame_bg,
            terrain_material,
            terrain_pipeline,
            depth: DepthTexture::new(device, config, "terrain-depth"),
            terrain_chunk_meshes: HashMap::new(),
            tree_chunk_meshes: HashMap::new(),
            house_chunk_meshes: HashMap::new(),
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) {
        self.depth = DepthTexture::new(device, config, "terrain-depth");
    }

    pub fn update_frame(
        &self,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        camera_position: Vec3,
        elapsed: f32,
        hour: f32,
    ) {
        self.frame_bg.update(
            queue,
            &FrameUniform::new(view_proj, camera_position, elapsed, hour),
        );
    }

    pub fn update_material(&self, queue: &wgpu::Queue, light_direction: Vec3, ambient: f32) {
        self.terrain_material
            .update_terrain(queue, light_direction, ambient);
    }

    pub fn sync_chunks(&mut self, device: &wgpu::Device, chunks: &HashMap<IVec2, ChunkData>) {
        self.terrain_chunk_meshes
            .retain(|coord, _| chunks.contains_key(coord));
        self.tree_chunk_meshes
            .retain(|coord, _| chunks.contains_key(coord));
        self.house_chunk_meshes
            .retain(|coord, _| chunks.contains_key(coord));

        for (coord, chunk) in chunks {
            if !self.terrain_chunk_meshes.contains_key(coord) {
                if let Some(gpu) = build_terrain_mesh(&chunk.terrain, &chunk.biome_map)
                    .and_then(|m| upload_mesh(device, &m, "terrain"))
                {
                    self.terrain_chunk_meshes.insert(*coord, gpu);
                }
            }

            if !self.tree_chunk_meshes.contains_key(coord) {
                if let Some(gpu) = build_tree_mesh(&chunk.content.trees)
                    .and_then(|m| upload_mesh(device, &m, "tree"))
                {
                    self.tree_chunk_meshes.insert(*coord, gpu);
                }
            }

            if !self.house_chunk_meshes.contains_key(coord) {
                if let Some(gpu) = build_house_mesh(&chunk.content.houses)
                    .and_then(|m| upload_mesh(device, &m, "house"))
                {
                    self.house_chunk_meshes.insert(*coord, gpu);
                }
            }
        }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_bind_group(0, &self.frame_bg.bind_group, &[]);

        pass.set_pipeline(&self.terrain_pipeline);
        pass.set_bind_group(1, &self.terrain_material.bind_group, &[]);

        for mesh in self.terrain_chunk_meshes.values() {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }

        for mesh in self.tree_chunk_meshes.values() {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }

        for mesh in self.house_chunk_meshes.values() {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }

    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth.view
    }
}

fn upload_mesh(device: &wgpu::Device, mesh: &CpuChunkMesh, label: &str) -> Option<GpuChunk> {
    if mesh.indices.is_empty() {
        return None;
    }

    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{label}-vertex-buffer")),
        contents: bytemuck::cast_slice(mesh.vertices.as_slice()),
        usage: wgpu::BufferUsages::VERTEX,
    });

    let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(&format!("{label}-index-buffer")),
        contents: bytemuck::cast_slice(mesh.indices.as_slice()),
        usage: wgpu::BufferUsages::INDEX,
    });

    Some(GpuChunk {
        vertex_buffer,
        index_buffer,
        index_count: mesh.indices.len() as u32,
    })
}
