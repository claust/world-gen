use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;

use super::hud_font::{self, HudVertex, ATLAS_H, ATLAS_W};
use super::pipeline::DEPTH_FORMAT;

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct HudUniform {
    screen_size: [f32; 2],
    _pad: [f32; 2],
}

const INITIAL_VERTEX_CAP: usize = 4096;

pub struct HudPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    font_bind_group: wgpu::BindGroup,
    vertex_buffer: wgpu::Buffer,
    vertex_cap: usize,
    vertex_count: u32,
}

impl HudPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &wgpu::SurfaceConfiguration,
    ) -> Self {
        // --- Uniform bind group (group 0) ---
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud-uniform-layout"),
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
            label: Some("hud-uniform-buffer"),
            contents: bytemuck::cast_slice(&[HudUniform {
                screen_size: [config.width as f32, config.height as f32],
                _pad: [0.0; 2],
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud-uniform-bg"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        // --- Font texture bind group (group 1) ---
        let font_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud-font-layout"),
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

        let atlas_pixels = hud_font::generate_atlas_pixels();
        let atlas_size = wgpu::Extent3d {
            width: ATLAS_W,
            height: ATLAS_H,
            depth_or_array_layers: 1,
        };
        let font_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hud-font-atlas"),
            size: atlas_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &font_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas_pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ATLAS_W),
                rows_per_image: Some(ATLAS_H),
            },
            atlas_size,
        );

        let font_view = font_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let font_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hud-font-sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let font_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud-font-bg"),
            layout: &font_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&font_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&font_sampler),
                },
            ],
        });

        // --- Pipeline ---
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hud-pipeline-layout"),
            bind_group_layouts: &[&uniform_layout, &font_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hud-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/hud.wgsl").into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud-pipeline"),
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
            label: Some("hud-vertex-buffer"),
            size: (INITIAL_VERTEX_CAP * std::mem::size_of::<HudVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            font_bind_group,
            vertex_buffer,
            vertex_cap: INITIAL_VERTEX_CAP,
            vertex_count: 0,
        }
    }

    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        camera_pos: Vec3,
        camera_yaw: f32,
        screen_w: f32,
        screen_h: f32,
    ) {
        // Update screen-size uniform
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[HudUniform {
                screen_size: [screen_w, screen_h],
                _pad: [0.0; 2],
            }]),
        );

        let mut verts: Vec<HudVertex> = Vec::with_capacity(512);

        // --- Coordinate text (top-left) ---
        let scale = 2.0;
        let pad = 10.0;
        let line_h = hud_font::GLYPH_H as f32 * scale + 4.0;
        let text_color = [1.0, 1.0, 1.0, 0.9];

        let line_x = format!("X: {:.1}m", camera_pos.x);
        let line_y = format!("Y: {:.1}m", camera_pos.y);
        let line_z = format!("Z: {:.1}m", camera_pos.z);

        hud_font::build_text_quads(&line_x, pad, pad, scale, text_color, &mut verts);
        hud_font::build_text_quads(&line_y, pad, pad + line_h, scale, text_color, &mut verts);
        hud_font::build_text_quads(
            &line_z,
            pad,
            pad + line_h * 2.0,
            scale,
            text_color,
            &mut verts,
        );

        // --- Compass rose (top-right) ---
        build_compass(&mut verts, camera_yaw, screen_w);

        // Upload vertices
        self.vertex_count = verts.len() as u32;
        let byte_size = verts.len() * std::mem::size_of::<HudVertex>();

        if verts.len() > self.vertex_cap {
            self.vertex_cap = verts.len().next_power_of_two();
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hud-vertex-buffer"),
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
        pass.set_bind_group(1, &self.font_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..self.vertex_count, 0..1);
    }
}

/// Sentinel UV marking a vertex as solid-color (no texture sampling).
const NO_UV: [f32; 2] = [-1.0, -1.0];

fn build_compass(verts: &mut Vec<HudVertex>, yaw: f32, screen_w: f32) {
    let cx = screen_w - 70.0;
    let cy = 70.0;
    let radius = 28.0;
    let label_radius = 44.0;

    let sin_y = (-yaw).sin();
    let cos_y = (-yaw).cos();

    // Rotate a point around (cx, cy)
    let rot = |angle: f32, r: f32| -> [f32; 2] {
        let dx = angle.sin() * r;
        let dy = -angle.cos() * r;
        let rx = dx * cos_y - dy * sin_y;
        let ry = dx * sin_y + dy * cos_y;
        [cx + rx, cy + ry]
    };

    // Diamond corners: north(0), east(π/2), south(π), west(3π/2)
    let n = rot(0.0, radius);
    let e = rot(std::f32::consts::FRAC_PI_2, radius);
    let s = rot(std::f32::consts::PI, radius);
    let w = rot(3.0 * std::f32::consts::FRAC_PI_2, radius);
    let center = [cx, cy];

    let north_color = [0.9, 0.15, 0.1, 0.9]; // bright red
    let south_color = [0.35, 0.1, 0.08, 0.9]; // dark

    // 4 triangles forming diamond
    let tri = |a: [f32; 2], b: [f32; 2], c: [f32; 2], color: [f32; 4]| -> [HudVertex; 3] {
        [
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
        ]
    };

    verts.extend_from_slice(&tri(center, w, n, north_color));
    verts.extend_from_slice(&tri(center, n, e, north_color));
    verts.extend_from_slice(&tri(center, e, s, south_color));
    verts.extend_from_slice(&tri(center, s, w, south_color));

    // Cardinal labels
    let label_color = [1.0, 1.0, 1.0, 0.95];
    let label_scale = 1.5;
    let gw = hud_font::GLYPH_W as f32 * label_scale;
    let gh = hud_font::GLYPH_H as f32 * label_scale;

    for (angle, label) in [
        (0.0, "N"),
        (std::f32::consts::FRAC_PI_2, "E"),
        (std::f32::consts::PI, "S"),
        (3.0 * std::f32::consts::FRAC_PI_2, "W"),
    ] {
        let pos = rot(angle, label_radius);
        // Center the glyph on that position
        let lx = pos[0] - gw * 0.5;
        let ly = pos[1] - gh * 0.5;
        hud_font::build_text_quads(label, lx, ly, label_scale, label_color, verts);
    }
}
