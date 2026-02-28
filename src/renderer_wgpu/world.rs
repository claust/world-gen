use std::collections::HashMap;

use glam::{IVec2, Mat4, Vec3};

use super::geometry::Vertex;
use super::instancing::{
    build_canopy_instances, build_house_instances, build_trunk_instances, upload_instances,
    GpuInstanceChunk, InstanceData, PrototypeMeshes,
};
use super::material::{FrameBindGroup, FrameUniform, MaterialBindGroup};
use super::terrain_compute::{GpuTerrainChunk, TerrainComputePipeline};
use crate::renderer_wgpu::pipeline::DepthTexture;
use crate::world_core::chunk::{ChunkData, CHUNK_GRID_RESOLUTION};

pub struct WorldRenderer {
    frame_bg: FrameBindGroup,
    terrain_material: MaterialBindGroup,
    terrain_pipeline: wgpu::RenderPipeline,
    instanced_pipeline: wgpu::RenderPipeline,
    terrain_compute: TerrainComputePipeline,
    prototypes: PrototypeMeshes,
    depth: DepthTexture,
    terrain_chunks: HashMap<IVec2, GpuTerrainChunk>,
    trunk_instances: HashMap<IVec2, GpuInstanceChunk>,
    canopy_instances: HashMap<IVec2, GpuInstanceChunk>,
    house_instances: HashMap<IVec2, GpuInstanceChunk>,
}

impl WorldRenderer {
    pub fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Self {
        let frame_bg = FrameBindGroup::new(device);
        let terrain_material = MaterialBindGroup::new_terrain(device);

        let terrain_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/terrain.wgsl").into()),
        });

        let instanced_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/instanced.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shared-pipeline-layout"),
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

        let terrain_pipeline = create_render_pipeline(
            device,
            config,
            &pipeline_layout,
            &terrain_shader,
            std::slice::from_ref(&vertex_layout),
            "terrain-pipeline",
        );

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

        let instanced_pipeline = create_render_pipeline(
            device,
            config,
            &pipeline_layout,
            &instanced_shader,
            &[vertex_layout, instance_layout],
            "instanced-pipeline",
        );

        let terrain_compute = TerrainComputePipeline::new(device);
        let prototypes = PrototypeMeshes::new(device);

        Self {
            frame_bg,
            terrain_material,
            terrain_pipeline,
            instanced_pipeline,
            terrain_compute,
            prototypes,
            depth: DepthTexture::new(device, config, "terrain-depth"),
            terrain_chunks: HashMap::new(),
            trunk_instances: HashMap::new(),
            canopy_instances: HashMap::new(),
            house_instances: HashMap::new(),
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

    pub fn sync_chunks(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunks: &HashMap<IVec2, ChunkData>,
    ) {
        self.terrain_chunks
            .retain(|coord, _| chunks.contains_key(coord));
        self.trunk_instances
            .retain(|coord, _| chunks.contains_key(coord));
        self.canopy_instances
            .retain(|coord, _| chunks.contains_key(coord));
        self.house_instances
            .retain(|coord, _| chunks.contains_key(coord));

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terrain-gen-encoder"),
        });
        let mut dispatched = false;

        for (coord, chunk) in chunks {
            if !self.terrain_chunks.contains_key(coord) {
                let total = CHUNK_GRID_RESOLUTION * CHUNK_GRID_RESOLUTION;
                if chunk.terrain.heights.len() == total
                    && chunk.terrain.moisture.len() == total
                    && chunk.terrain.max_height >= chunk.terrain.min_height
                {
                    let gpu = self.terrain_compute.generate_chunk(
                        device,
                        &mut encoder,
                        *coord,
                        &chunk.terrain.heights,
                        &chunk.terrain.moisture,
                    );
                    self.terrain_chunks.insert(*coord, gpu);
                    dispatched = true;
                }
            }

            if !self.trunk_instances.contains_key(coord) {
                let data = build_trunk_instances(&chunk.content.trees);
                if let Some(gpu) = upload_instances(device, &data, "trunk") {
                    self.trunk_instances.insert(*coord, gpu);
                }
            }

            if !self.canopy_instances.contains_key(coord) {
                let data = build_canopy_instances(&chunk.content.trees);
                if let Some(gpu) = upload_instances(device, &data, "canopy") {
                    self.canopy_instances.insert(*coord, gpu);
                }
            }

            if !self.house_instances.contains_key(coord) {
                let data = build_house_instances(&chunk.content.houses);
                if let Some(gpu) = upload_instances(device, &data, "house") {
                    self.house_instances.insert(*coord, gpu);
                }
            }
        }

        if dispatched {
            queue.submit(Some(encoder.finish()));
        }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_bind_group(0, &self.frame_bg.bind_group, &[]);
        pass.set_bind_group(1, &self.terrain_material.bind_group, &[]);

        // Terrain: shared index buffer, per-chunk compute-generated vertex buffer
        pass.set_pipeline(&self.terrain_pipeline);
        pass.set_index_buffer(
            self.terrain_compute.shared_index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        for chunk in self.terrain_chunks.values() {
            pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
            pass.draw_indexed(0..self.terrain_compute.shared_index_count, 0, 0..1);
        }

        // Instanced objects
        pass.set_pipeline(&self.instanced_pipeline);

        // Tree trunks
        pass.set_vertex_buffer(0, self.prototypes.unit_box.vertex_buffer.slice(..));
        pass.set_index_buffer(
            self.prototypes.unit_box.index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        for inst in self.trunk_instances.values() {
            pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
            pass.draw_indexed(
                0..self.prototypes.unit_box.index_count,
                0,
                0..inst.instance_count,
            );
        }

        // Tree canopies
        pass.set_vertex_buffer(0, self.prototypes.unit_octahedron.vertex_buffer.slice(..));
        pass.set_index_buffer(
            self.prototypes.unit_octahedron.index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        for inst in self.canopy_instances.values() {
            pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
            pass.draw_indexed(
                0..self.prototypes.unit_octahedron.index_count,
                0,
                0..inst.instance_count,
            );
        }

        // Houses
        pass.set_vertex_buffer(0, self.prototypes.house.vertex_buffer.slice(..));
        pass.set_index_buffer(
            self.prototypes.house.index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        for inst in self.house_instances.values() {
            pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
            pass.draw_indexed(
                0..self.prototypes.house.index_count,
                0,
                0..inst.instance_count,
            );
        }
    }

    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth.view
    }
}

fn create_render_pipeline(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vertex_buffers: &[wgpu::VertexBufferLayout<'_>],
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: "vs_main",
            buffers: vertex_buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
    })
}
