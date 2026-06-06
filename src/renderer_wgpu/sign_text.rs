//! World-space text for the debug tile-marker road-post signs.
//!
//! Reuses the MSDF glyph atlas from [`super::hud_font`], but instead of
//! drawing in screen space it bakes each chunk's coordinate label ("X3 Z-2")
//! into a small static vertex buffer positioned on the sign board in world
//! space. The board is a fixed flat panel (no billboarding); text is painted on
//! both faces so it reads from either side, and the opaque board between the two
//! text planes hides the back face from a front-side viewer.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{IVec2, Vec3};
use wgpu::util::DeviceExt;

use super::frustum::Frustum;
use super::hud_font::MsdfFont;
use super::instancing::{SIGN_BOARD_CENTER_Y, SIGN_BOARD_HALF_T, SIGN_BOARD_HALF_W};
use super::pipeline::DEPTH_FORMAT;
use crate::world_core::chunk::{ChunkData, CHUNK_SIZE_METERS};

/// Glyph height on the board, in meters.
const TEXT_HEIGHT_M: f32 = 0.30;
/// Color of the painted text (near-black for contrast on the light board).
const TEXT_COLOR: [f32; 4] = [0.12, 0.10, 0.10, 1.0];
/// Labels farther than this (squared, meters) from the camera are skipped — they
/// are unreadable at distance and would only clutter the view.
const LABEL_MAX_DIST_SQ: f32 = 900.0 * 900.0;

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct SignVertex {
    position: [f32; 3],
    uv: [f32; 2],
    color: [f32; 4],
}

impl SignVertex {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Self>() as u64,
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
                format: wgpu::VertexFormat::Float32x4,
            },
        ],
    };
}

struct ChunkSign {
    buffer: wgpu::Buffer,
    vertex_count: u32,
}

pub struct SignTextPass {
    pipeline: wgpu::RenderPipeline,
    font_bind_group: wgpu::BindGroup,
    font: MsdfFont,
    signs: HashMap<IVec2, ChunkSign>,
}

impl SignTextPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        render_format: wgpu::TextureFormat,
        frame_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        // --- Font texture bind group (group 1) ---
        let font_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sign-font-layout"),
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

        let (font, atlas_rgba) = MsdfFont::load();
        let atlas_size = wgpu::Extent3d {
            width: font.atlas_w,
            height: font.atlas_h,
            depth_or_array_layers: 1,
        };
        let font_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("sign-font-atlas"),
            size: atlas_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &font_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &atlas_rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(font.atlas_w * 4),
                rows_per_image: Some(font.atlas_h),
            },
            atlas_size,
        );
        // The CPU texels live only on the GPU now; metrics are kept in `font`.
        drop(atlas_rgba);

        let font_view = font_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Linear filtering: the MSDF shader reconstructs each glyph's edge from a
        // smoothly interpolated distance, so the sampler must interpolate. This also
        // keeps small/grazing-angle text stable instead of shimmering.
        let font_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sign-font-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let font_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sign-font-bg"),
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

        // --- Pipeline (group 0 = frame uniform, group 1 = font) ---
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sign-text-pipeline-layout"),
            bind_group_layouts: &[frame_layout, &font_layout],
            push_constant_ranges: &[],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sign-text-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sign_text.wgsl").into()),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sign-text-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[SignVertex::LAYOUT],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                // Text quads are flat; both faces are emitted explicitly so we
                // never cull.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        Self {
            pipeline,
            font_bind_group,
            font,
            signs: HashMap::new(),
        }
    }

    /// Retain only loaded chunks; build a label buffer for any new chunk. Labels
    /// are static per chunk, so each is built exactly once.
    pub fn sync_chunks(&mut self, device: &wgpu::Device, chunks: &HashMap<IVec2, ChunkData>) {
        self.signs.retain(|coord, _| chunks.contains_key(coord));

        for (coord, chunk) in chunks {
            if self.signs.contains_key(coord) {
                continue;
            }
            let half = CHUNK_SIZE_METERS * 0.5;
            let ground = chunk.terrain.height_at_world(half, half);
            let verts = build_label_vertices(&self.font, *coord, ground);
            if verts.is_empty() {
                continue;
            }
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("sign-text-vb"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.signs.insert(
                *coord,
                ChunkSign {
                    buffer,
                    vertex_count: verts.len() as u32,
                },
            );
        }
    }

    /// Draw all visible, nearby labels. Assumes the caller has bound the frame
    /// uniform at group 0.
    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frustum: &Frustum,
        camera_position: Vec3,
    ) {
        if self.signs.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(1, &self.font_bind_group, &[]);

        for (coord, sign) in &self.signs {
            if !frustum.is_chunk_visible(*coord) {
                continue;
            }
            let dx = (coord.x as f32 + 0.5) * CHUNK_SIZE_METERS - camera_position.x;
            let dz = (coord.y as f32 + 0.5) * CHUNK_SIZE_METERS - camera_position.z;
            if dx * dx + dz * dz > LABEL_MAX_DIST_SQ {
                continue;
            }
            pass.set_vertex_buffer(0, sign.buffer.slice(..));
            pass.draw(0..sign.vertex_count, 0..1);
        }
    }
}

/// Build the world-space text geometry for one chunk's sign. Two faces are
/// emitted: a front face readable from +Z and a mirrored back face readable
/// from -Z.
fn build_label_vertices(font: &MsdfFont, coord: IVec2, ground_y: f32) -> Vec<SignVertex> {
    let text = format!("X{} Z{}", coord.x, coord.y);

    // Approximate uppercase cap height, in em. The label is sized so this band is
    // `TEXT_HEIGHT_M` tall, then shrunk to fit the board if the proportional layout
    // would overflow (e.g. "X-10 Z-10").
    const CAP_EM: f32 = 0.72;
    let mut em_m = TEXT_HEIGHT_M / CAP_EM; // meters per em
    let usable_w = 2.0 * SIGN_BOARD_HALF_W * 0.92;
    let text_w_em = font.measure(&text, 1.0); // width in em (px = 1 → em units)
    let natural_w = text_w_em * em_m;
    if natural_w > usable_w {
        em_m *= usable_w / natural_w;
    }
    let start_x = -text_w_em * em_m * 0.5;

    let cx = (coord.x as f32 + 0.5) * CHUNK_SIZE_METERS;
    let cz = (coord.y as f32 + 0.5) * CHUNK_SIZE_METERS;
    let cy = ground_y + SIGN_BOARD_CENTER_Y;
    let z_off = SIGN_BOARD_HALF_T + 0.012;
    // Baseline so the cap band centers vertically on the board center. Glyphs are
    // placed y-up (`yt = baseline + plane.top * em_m`), so the cap band spans
    // `[baseline, baseline + CAP_EM*em_m]` and its midpoint sits half a cap above
    // the baseline — subtract that to land the midpoint on `cy`.
    let baseline = cy - CAP_EM * 0.5 * em_m;

    let mut out = Vec::with_capacity(text.len() * 12);
    let mut pen = start_x;
    for c in text.chars() {
        let Some(g) = font.glyph(c) else {
            continue;
        };
        if g.is_blank() {
            pen += g.advance * em_m;
            continue;
        }
        // Plane is y-up relative to the baseline (matches world +Y).
        let lx0 = pen + g.plane[0] * em_m;
        let lx1 = pen + g.plane[2] * em_m;
        let yb = baseline + g.plane[1] * em_m;
        let yt = baseline + g.plane[3] * em_m;
        let [u0, v0, u1, v1] = g.uv;

        // Front face (+Z): world +X is the reader's left-to-right.
        push_quad(
            &mut out,
            [
                [cx + lx0, yt, cz + z_off],
                [cx + lx1, yt, cz + z_off],
                [cx + lx0, yb, cz + z_off],
                [cx + lx1, yb, cz + z_off],
            ],
            [[u0, v0], [u1, v0], [u0, v1], [u1, v1]],
        );

        // Back face (-Z): mirrored along X so it reads correctly from behind.
        push_quad(
            &mut out,
            [
                [cx - lx0, yt, cz - z_off],
                [cx - lx1, yt, cz - z_off],
                [cx - lx0, yb, cz - z_off],
                [cx - lx1, yb, cz - z_off],
            ],
            [[u0, v0], [u1, v0], [u0, v1], [u1, v1]],
        );

        pen += g.advance * em_m;
    }

    out
}

/// Push two triangles for a quad given corners [TL, TR, BL, BR] and matching UVs.
fn push_quad(out: &mut Vec<SignVertex>, corners: [[f32; 3]; 4], uvs: [[f32; 2]; 4]) {
    let v = |idx: usize| SignVertex {
        position: corners[idx],
        uv: uvs[idx],
        color: TEXT_COLOR,
    };
    // TL, TR, BL then BL, TR, BR
    out.push(v(0));
    out.push(v(1));
    out.push(v(2));
    out.push(v(2));
    out.push(v(1));
    out.push(v(3));
}
