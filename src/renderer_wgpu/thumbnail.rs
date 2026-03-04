use bytemuck::cast_slice;
use glam::{Mat4, Vec3};

use super::geometry::Vertex;
use super::instancing::{upload_instances, upload_prototype, InstanceData, PrototypeMesh};
use super::material::{FrameBindGroup, FrameUniform, MaterialBindGroup, TerrainMaterialUniform};
use super::pipeline::{create_render_pipeline, DEPTH_FORMAT};
use crate::world_core::herbarium::Herbarium;
use crate::world_core::plant_gen;

const THUMBNAIL_SIZE: u32 = 128;
const THUMBNAIL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Minimum extent for camera framing to avoid degenerate matrices.
const MIN_EXTENT: f32 = 0.1;

pub struct ThumbnailRenderer {
    pipeline: wgpu::RenderPipeline,
    frame_bind: FrameBindGroup,
    material_bind: MaterialBindGroup,
    depth_view: wgpu::TextureView,
    textures: Vec<Option<egui::TextureId>>,
}

/// Temporary GPU resources for rendering a single thumbnail.
struct PlantRenderData {
    prototype: PrototypeMesh,
    instance: wgpu::Buffer,
    _color_tex: wgpu::Texture,
    color_view: wgpu::TextureView,
    frame_uniform: FrameUniform,
}

impl ThumbnailRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let frame_bind = FrameBindGroup::new(device);
        let material_bind = MaterialBindGroup::new_terrain(device);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("thumbnail-pipeline-layout"),
            bind_group_layouts: &[&frame_bind.layout, &material_bind.layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("thumbnail-shader"),
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
            THUMBNAIL_FORMAT,
            &pipeline_layout,
            &shader,
            &[vertex_layout, instance_layout],
            "thumbnail-pipeline",
        );

        let depth_view = create_depth_texture(device);

        Self {
            pipeline,
            frame_bind,
            material_bind,
            depth_view,
            textures: Vec::new(),
        }
    }

    pub fn generate_all(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        herbarium: &Herbarium,
        seed: u32,
        egui_renderer: &mut egui_wgpu::Renderer,
    ) {
        // Write material uniform
        write_thumbnail_material(queue, &self.material_bind);

        // Free old textures
        for id in self.textures.drain(..).flatten() {
            egui_renderer.free_texture(&id);
        }

        // Prepare all plants on CPU, collecting GPU resources
        let mut render_data: Vec<Option<PlantRenderData>> = Vec::new();
        for entry in &herbarium.plants {
            render_data.push(prepare_plant(device, &entry.species, seed));
        }

        // Encode all render passes into a single command buffer
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("thumb-encoder"),
        });

        for rd in render_data.iter().flatten() {
            self.frame_bind.update(queue, &rd.frame_uniform);
            self.encode_render_pass(&mut encoder, rd);
        }

        queue.submit(std::iter::once(encoder.finish()));

        // Register all textures with egui
        for data in &render_data {
            let tex_id = data.as_ref().map(|rd| {
                egui_renderer.register_native_texture(
                    device,
                    &rd.color_view,
                    wgpu::FilterMode::Linear,
                )
            });
            self.textures.push(tex_id);
        }
    }

    pub fn invalidate(
        &mut self,
        index: usize,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        herbarium: &Herbarium,
        seed: u32,
        egui_renderer: &mut egui_wgpu::Renderer,
    ) {
        // Ensure textures vec is big enough
        while self.textures.len() <= index {
            self.textures.push(None);
        }

        // Free old texture
        if let Some(Some(id)) = self.textures.get(index) {
            egui_renderer.free_texture(id);
        }

        if let Some(entry) = herbarium.plants.get(index) {
            write_thumbnail_material(queue, &self.material_bind);

            if let Some(rd) = prepare_plant(device, &entry.species, seed) {
                self.frame_bind.update(queue, &rd.frame_uniform);

                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("thumb-encoder-single"),
                });
                self.encode_render_pass(&mut encoder, &rd);
                queue.submit(std::iter::once(encoder.finish()));

                let tex_id = egui_renderer.register_native_texture(
                    device,
                    &rd.color_view,
                    wgpu::FilterMode::Linear,
                );
                self.textures[index] = Some(tex_id);
            } else {
                self.textures[index] = None;
            }
        } else {
            self.textures[index] = None;
        }
    }

    pub fn get_texture_id(&self, index: usize) -> Option<egui::TextureId> {
        self.textures.get(index).copied().flatten()
    }

    fn encode_render_pass(&self, encoder: &mut wgpu::CommandEncoder, rd: &PlantRenderData) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("thumb-render-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &rd.color_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 0.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.frame_bind.bind_group, &[]);
        pass.set_bind_group(1, &self.material_bind.bind_group, &[]);

        pass.set_vertex_buffer(0, rd.prototype.vertex_buffer.slice(..));
        pass.set_index_buffer(
            rd.prototype.index_buffer.slice(..),
            wgpu::IndexFormat::Uint32,
        );
        pass.set_vertex_buffer(1, rd.instance.slice(..));
        pass.draw_indexed(0..rd.prototype.index_count, 0, 0..1);
    }
}

fn write_thumbnail_material(queue: &wgpu::Queue, material_bind: &MaterialBindGroup) {
    let material_data = TerrainMaterialUniform {
        light_direction: [0.3, 0.9, 0.2, 0.0],
        ambient: [0.4, 0.4, 0.4, 0.0],
        fog_color: [0.0, 0.0, 0.0, 0.0],
        fog_params: [10000.0, 20000.0, 0.0, 0.0],
        sun_color: [1.0, 1.0, 0.95, 0.0],
        sky_zenith: [0.35, 0.55, 0.90, 0.0],
        sky_horizon: [0.55, 0.75, 0.95, 0.0],
    };
    queue.write_buffer(&material_bind.buffer, 0, cast_slice(&[material_data]));
}

/// Generate the plant mesh, upload GPU buffers, compute camera framing,
/// and create the offscreen color texture. Returns `None` for empty meshes.
fn prepare_plant(
    device: &wgpu::Device,
    species: &crate::world_core::plant_gen::config::SpeciesConfig,
    seed: u32,
) -> Option<PlantRenderData> {
    let plant_mesh = plant_gen::generate_plant_mesh(species, seed);
    if plant_mesh.vertices.is_empty() {
        return None;
    }

    // Convert PlantVertex -> Vertex
    let verts: Vec<Vertex> = plant_mesh
        .vertices
        .iter()
        .map(|v| Vertex {
            position: v.position,
            normal: v.normal,
            color: v.color,
        })
        .collect();

    let prototype = upload_prototype(device, &verts, &plant_mesh.indices, "thumb-proto");

    // Compute AABB for camera framing
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for v in &verts {
        let p = Vec3::from(v.position);
        min = min.min(p);
        max = max.max(p);
    }
    let center = (min + max) * 0.5;
    let extent = max - min;
    let max_dim = extent.x.max(extent.y).max(extent.z).max(MIN_EXTENT);

    // Camera at ~30° elevation, looking at center
    let distance = max_dim * 1.8;
    let elevation_angle: f32 = 30.0_f32.to_radians();
    let cam_pos = center
        + Vec3::new(
            distance * elevation_angle.cos(),
            distance * elevation_angle.sin(),
            distance * 0.3,
        );

    let view = Mat4::look_at_rh(cam_pos, center, Vec3::Y);
    let proj = Mat4::perspective_rh(
        45.0_f32.to_radians(),
        1.0, // square aspect
        0.1,
        (distance * 4.0).max(0.2),
    );
    let view_proj = proj * view;

    let frame_uniform = FrameUniform::new(view_proj, cam_pos, 0.0, 12.0);

    // Single instance at origin
    let instance_data = InstanceData {
        position: [0.0, 0.0, 0.0],
        rotation_y: 0.0,
        scale: [1.0, 1.0, 1.0],
        _pad: 0.0,
        color: [1.0, 1.0, 1.0, 1.0],
    };
    let instance = upload_instances(device, &[instance_data], "thumb-inst")?.instance_buffer;

    // Create offscreen color texture
    let color_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thumb-color"),
        size: wgpu::Extent3d {
            width: THUMBNAIL_SIZE,
            height: THUMBNAIL_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: THUMBNAIL_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let color_view = color_tex.create_view(&wgpu::TextureViewDescriptor::default());

    Some(PlantRenderData {
        prototype,
        instance,
        _color_tex: color_tex,
        color_view,
        frame_uniform,
    })
}

fn create_depth_texture(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thumb-depth"),
        size: wgpu::Extent3d {
            width: THUMBNAIL_SIZE,
            height: THUMBNAIL_SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}
