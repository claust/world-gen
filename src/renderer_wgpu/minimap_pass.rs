use std::collections::{HashMap, HashSet};

use bytemuck::{Pod, Zeroable};
use glam::IVec2;
use wgpu::util::DeviceExt;

use super::hud_font::HudVertex;
use super::minimap_colors::biome_color_rgba;
use super::pipeline::DEPTH_FORMAT;
use crate::world_core::chunk::{ChunkData, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS};

const MAP_TEX_SIZE: u32 = 256;
const MINIMAP_PX: f32 = 200.0;
const MINIMAP_MARGIN: f32 = 15.0;
const BORDER_WIDTH: f32 = 2.0;
const INITIAL_VERTEX_CAP: usize = 256;

/// Sentinel UV marking a vertex as solid-color (no texture sampling).
const NO_UV: [f32; 2] = [-1.0, -1.0];

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct MinimapUniform {
    screen_size: [f32; 2],
    _pad: [f32; 2],
}

pub struct MinimapPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    texture: wgpu::Texture,
    texture_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    vertex_cap: usize,
    vertex_count: u32,
    last_chunk_set: HashSet<IVec2>,
    /// World-space bounding box of loaded chunks (min corner).
    world_origin: [f32; 2],
    /// World-space extent (width, height) of loaded chunks.
    world_extent: [f32; 2],
}

impl MinimapPass {
    pub fn new(
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        config: &wgpu::SurfaceConfiguration,
    ) -> Self {
        // --- Uniform bind group (group 0) ---
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("minimap-uniform-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("minimap-uniform-buffer"),
            contents: bytemuck::cast_slice(&[MinimapUniform {
                screen_size: [config.width as f32, config.height as f32],
                _pad: [0.0; 2],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("minimap-uniform-bg"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // --- Map texture bind group (group 1) ---
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("minimap-texture-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("minimap-texture"),
            size: wgpu::Extent3d {
                width: MAP_TEX_SIZE,
                height: MAP_TEX_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("minimap-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("minimap-texture-bg"),
            layout: &texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        // --- Pipeline ---
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("minimap-pipeline-layout"),
            bind_group_layouts: &[&uniform_layout, &texture_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("minimap-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/minimap.wgsl").into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("minimap-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[HudVertex::LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::Always,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // --- Vertex buffer ---
        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("minimap-vertex-buffer"),
            size: (INITIAL_VERTEX_CAP * std::mem::size_of::<HudVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            texture,
            texture_bind_group,
            vertex_buffer,
            vertex_cap: INITIAL_VERTEX_CAP,
            vertex_count: 0,
            last_chunk_set: HashSet::new(),
            world_origin: [0.0; 2],
            world_extent: [1.0; 2],
        }
    }

    /// Check if the chunk set changed; if so, rasterize the texture.
    pub fn sync_chunks(&mut self, queue: &wgpu::Queue, chunks: &HashMap<IVec2, ChunkData>) {
        let current_keys: HashSet<IVec2> = chunks.keys().copied().collect();
        if current_keys == self.last_chunk_set {
            return;
        }
        self.last_chunk_set = current_keys;
        self.rasterize_texture(queue, chunks);
    }

    /// CPU-rasterize the 256x256 minimap texture from chunk heightmap data.
    fn rasterize_texture(&mut self, queue: &wgpu::Queue, chunks: &HashMap<IVec2, ChunkData>) {
        if chunks.is_empty() {
            return;
        }

        // Compute world bounding box of all loaded chunks
        let mut min_x = f32::MAX;
        let mut min_z = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_z = f32::MIN;

        for coord in chunks.keys() {
            let wx = coord.x as f32 * CHUNK_SIZE_METERS;
            let wz = coord.y as f32 * CHUNK_SIZE_METERS;
            min_x = min_x.min(wx);
            min_z = min_z.min(wz);
            max_x = max_x.max(wx + CHUNK_SIZE_METERS);
            max_z = max_z.max(wz + CHUNK_SIZE_METERS);
        }

        self.world_origin = [min_x, min_z];
        self.world_extent = [(max_x - min_x).max(1.0), (max_z - min_z).max(1.0)];

        let side = CHUNK_GRID_RESOLUTION;
        let mut pixels = vec![0u8; (MAP_TEX_SIZE * MAP_TEX_SIZE * 4) as usize];

        for py in 0..MAP_TEX_SIZE {
            for px in 0..MAP_TEX_SIZE {
                // Map pixel to world position
                // +X = north, +Z = east in world space
                // Minimap: right (px+) = east (+Z), up (py=0) = north (max X)
                let u = px as f32 / MAP_TEX_SIZE as f32;
                let v = py as f32 / MAP_TEX_SIZE as f32;

                let world_z = min_z + u * self.world_extent[1]; // right = east = +Z
                let world_x = max_x - v * self.world_extent[0]; // top = north = max X

                // Find which chunk this falls in
                let cx = (world_x / CHUNK_SIZE_METERS).floor() as i32;
                let cz = (world_z / CHUNK_SIZE_METERS).floor() as i32;

                let color = if let Some(chunk) = chunks.get(&IVec2::new(cx, cz)) {
                    // Local coords within chunk
                    let local_x = world_x - cx as f32 * CHUNK_SIZE_METERS;
                    let local_z = world_z - cz as f32 * CHUNK_SIZE_METERS;

                    let xf = (local_x / CHUNK_SIZE_METERS * (side - 1) as f32)
                        .clamp(0.0, (side - 1) as f32);
                    let zf = (local_z / CHUNK_SIZE_METERS * (side - 1) as f32)
                        .clamp(0.0, (side - 1) as f32);

                    let xi = (xf as usize).min(side - 1);
                    let zi = (zf as usize).min(side - 1);
                    let idx = zi * side + xi;

                    let h = chunk.terrain.heights[idx];
                    let m = chunk.terrain.moisture[idx];
                    biome_color_rgba(h, m)
                } else {
                    [20, 20, 30, 255] // unloaded = dark
                };

                let offset = ((py * MAP_TEX_SIZE + px) * 4) as usize;
                pixels[offset..offset + 4].copy_from_slice(&color);
            }
        }

        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(MAP_TEX_SIZE * 4),
                rows_per_image: Some(MAP_TEX_SIZE),
            },
            wgpu::Extent3d {
                width: MAP_TEX_SIZE,
                height: MAP_TEX_SIZE,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Rebuild overlay vertices each frame (border, textured quad, FOV cone, camera dot).
    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        camera_pos: glam::Vec3,
        camera_yaw: f32,
        camera_fov: f32,
        screen_w: f32,
        screen_h: f32,
    ) {
        // Update screen-size uniform
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[MinimapUniform {
                screen_size: [screen_w, screen_h],
                _pad: [0.0; 2],
            }]),
        );

        let mut verts: Vec<HudVertex> = Vec::with_capacity(64);

        // Minimap placement: bottom-left corner
        let map_x = MINIMAP_MARGIN;
        let map_y = screen_h - MINIMAP_MARGIN - MINIMAP_PX;

        // --- Dark border background ---
        let border_color = [0.1, 0.1, 0.12, 0.85];
        let bx = map_x - BORDER_WIDTH;
        let by = map_y - BORDER_WIDTH;
        let bw = MINIMAP_PX + BORDER_WIDTH * 2.0;
        let bh = MINIMAP_PX + BORDER_WIDTH * 2.0;
        push_solid_quad(&mut verts, bx, by, bw, bh, border_color);

        // --- Textured map quad ---
        let map_color = [1.0, 1.0, 1.0, 0.92]; // slight transparency
        push_textured_quad(&mut verts, map_x, map_y, MINIMAP_PX, MINIMAP_PX, map_color);

        // --- Camera position on minimap ---
        // +X = north (up on minimap), +Z = east (right on minimap)
        if self.world_extent[0] > 0.0 && self.world_extent[1] > 0.0 {
            // Horizontal: Z maps to east (right)
            let cam_u = (camera_pos.z - self.world_origin[1]) / self.world_extent[1];
            // Vertical: X maps to north (up), flip so top = max X
            let cam_v = 1.0 - (camera_pos.x - self.world_origin[0]) / self.world_extent[0];

            let cam_map_x = map_x + cam_u.clamp(0.0, 1.0) * MINIMAP_PX;
            let cam_map_y = map_y + cam_v.clamp(0.0, 1.0) * MINIMAP_PX;

            // --- FOV cone ---
            let cone_len = MINIMAP_PX * 0.4;
            let half_fov = camera_fov / 2.0;

            // World forward at yaw: (cos(yaw), 0, sin(yaw))
            // On minimap: +Z → right (screen +X), +X → up (screen -Y)
            // So screen_dx = sin(yaw), screen_dy = -cos(yaw)
            let fwd_x = camera_yaw.sin();
            let fwd_y = -camera_yaw.cos();

            let left_angle = camera_yaw - half_fov;
            let right_angle = camera_yaw + half_fov;

            let left_x = cam_map_x + left_angle.sin() * cone_len;
            let left_y = cam_map_y + (-left_angle.cos()) * cone_len;
            let right_x = cam_map_x + right_angle.sin() * cone_len;
            let right_y = cam_map_y + (-right_angle.cos()) * cone_len;

            // Mid-point along forward for a second triangle to fill the cone
            let mid_x = cam_map_x + fwd_x * cone_len;
            let mid_y = cam_map_y + fwd_y * cone_len;

            let cone_color = [1.0, 1.0, 1.0, 0.30];

            // Two triangles to fill the FOV cone
            push_solid_tri(
                &mut verts,
                [cam_map_x, cam_map_y],
                [left_x, left_y],
                [mid_x, mid_y],
                cone_color,
            );
            push_solid_tri(
                &mut verts,
                [cam_map_x, cam_map_y],
                [mid_x, mid_y],
                [right_x, right_y],
                cone_color,
            );

            // --- Camera dot ---
            let dot_size = 4.0;
            let dot_color = [0.95, 0.35, 0.1, 1.0]; // orange-red
            push_solid_quad(
                &mut verts,
                cam_map_x - dot_size / 2.0,
                cam_map_y - dot_size / 2.0,
                dot_size,
                dot_size,
                dot_color,
            );
        }

        // Upload vertices
        self.vertex_count = verts.len() as u32;
        let byte_size = verts.len() * std::mem::size_of::<HudVertex>();

        if verts.len() > self.vertex_cap {
            self.vertex_cap = verts.len().next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("minimap-vertex-buffer"),
                size: (self.vertex_cap * std::mem::size_of::<HudVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }

        if byte_size > 0 {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&verts));
        }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if self.vertex_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        pass.set_bind_group(1, &self.texture_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

/// Push a solid-color quad (2 triangles, 6 vertices).
fn push_solid_quad(verts: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
    let tl = [x, y];
    let tr = [x + w, y];
    let bl = [x, y + h];
    let br = [x + w, y + h];

    verts.extend_from_slice(&[
        HudVertex {
            position: tl,
            uv: NO_UV,
            color,
        },
        HudVertex {
            position: tr,
            uv: NO_UV,
            color,
        },
        HudVertex {
            position: bl,
            uv: NO_UV,
            color,
        },
        HudVertex {
            position: bl,
            uv: NO_UV,
            color,
        },
        HudVertex {
            position: tr,
            uv: NO_UV,
            color,
        },
        HudVertex {
            position: br,
            uv: NO_UV,
            color,
        },
    ]);
}

/// Push a textured quad (2 triangles, 6 vertices) with UV [0,0]→[1,1].
fn push_textured_quad(verts: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
    let tl = [x, y];
    let tr = [x + w, y];
    let bl = [x, y + h];
    let br = [x + w, y + h];

    verts.extend_from_slice(&[
        HudVertex {
            position: tl,
            uv: [0.0, 0.0],
            color,
        },
        HudVertex {
            position: tr,
            uv: [1.0, 0.0],
            color,
        },
        HudVertex {
            position: bl,
            uv: [0.0, 1.0],
            color,
        },
        HudVertex {
            position: bl,
            uv: [0.0, 1.0],
            color,
        },
        HudVertex {
            position: tr,
            uv: [1.0, 0.0],
            color,
        },
        HudVertex {
            position: br,
            uv: [1.0, 1.0],
            color,
        },
    ]);
}

/// Push a solid-color triangle (3 vertices).
fn push_solid_tri(
    verts: &mut Vec<HudVertex>,
    a: [f32; 2],
    b: [f32; 2],
    c: [f32; 2],
    color: [f32; 4],
) {
    verts.extend_from_slice(&[
        HudVertex {
            position: a,
            uv: NO_UV,
            color,
        },
        HudVertex {
            position: b,
            uv: NO_UV,
            color,
        },
        HudVertex {
            position: c,
            uv: NO_UV,
            color,
        },
    ]);
}
