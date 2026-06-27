use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::hud_font::{self, HudVertex, MsdfFont, SDF_PLAIN};
use super::pipeline::DEPTH_FORMAT;

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct HudUniform {
    screen_size: [f32; 2],
    /// MSDF distance-field range, in atlas texels (forwarded to the shader's AA).
    px_range: f32,
    _pad: f32,
}

const INITIAL_VERTEX_CAP: usize = 4096;

/// Font size for the compass cardinal labels.
const COMPASS_LABEL_PX: f32 = 20.0;

pub struct HudPass {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    font_bind_group: wgpu::BindGroup,
    font: MsdfFont,
    /// Width / height of the plant-count backdrop image, so its quad keeps aspect.
    backdrop_aspect: f32,
    vertex_buffer: wgpu::Buffer,
    vertex_cap: usize,
    vertex_count: u32,
}

impl HudPass {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        render_format: wgpu::TextureFormat,
    ) -> Self {
        // --- Uniform bind group (group 0) ---
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud-uniform-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // VERTEX projects positions; FRAGMENT reads px_range for MSDF AA.
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let (font, atlas_rgba) = MsdfFont::load();

        // Plant-readout brand mark: the flat FloraForge spruce, drawn as a translucent
        // motif with the population count seated inside its lower canopy. Its width is a
        // multiple of 64 so `bytes_per_row` (w * 4) meets wgpu's 256-byte copy alignment.
        let backdrop_png = include_bytes!("../../assets/hud/plant_count.png");
        let backdrop = image::load_from_memory(backdrop_png)
            .expect("decode plant_count.png")
            .to_rgba8();
        let (backdrop_w, backdrop_h) = backdrop.dimensions();
        // `write_texture` requires `bytes_per_row` (w * 4) to be 256-byte aligned, i.e.
        // the width a multiple of 64. Assert it so swapping in a differently-sized asset
        // fails here with a clear message rather than as an opaque wgpu validation error.
        assert!(
            backdrop_w.is_multiple_of(64),
            "plant_count.png width ({backdrop_w}) must be a multiple of 64 for the texture row pitch to meet wgpu's 256-byte copy alignment",
        );
        let backdrop_aspect = backdrop_w as f32 / backdrop_h as f32;

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hud-uniform-buffer"),
            contents: bytemuck::cast_slice(&[HudUniform {
                screen_size: [1.0, 1.0],
                px_range: font.px_range,
                _pad: 0.0,
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
                // Plant-count backdrop image; shares binding 1's sampler.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let atlas_size = wgpu::Extent3d {
            width: font.atlas_w,
            height: font.atlas_h,
            depth_or_array_layers: 1,
        };
        let font_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hud-font-atlas"),
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

        // --- Backdrop texture ---
        // sRGB so the brand-green is interpreted correctly; the MSDF atlas above is
        // a non-color distance field and stays Unorm.
        let backdrop_size = wgpu::Extent3d {
            width: backdrop_w,
            height: backdrop_h,
            depth_or_array_layers: 1,
        };
        let backdrop_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hud-plant-backdrop"),
            size: backdrop_size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &backdrop_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &backdrop,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(backdrop_w * 4),
                rows_per_image: Some(backdrop_h),
            },
            backdrop_size,
        );
        let backdrop_view = backdrop_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let font_view = font_texture.create_view(&wgpu::TextureViewDescriptor::default());
        // Linear filtering: the MSDF shader reconstructs the edge from a smoothly
        // interpolated distance, so the sampler must interpolate (not snap).
        let font_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hud-font-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
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
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&backdrop_view),
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
                    format: render_format,
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
            font,
            backdrop_aspect,
            vertex_buffer,
            vertex_cap: INITIAL_VERTEX_CAP,
            vertex_count: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        camera_yaw: f32,
        hour: f32,
        fps: f32,
        plant_count: usize,
        day_number: u64,
        weekly_delta: i64,
        screen_w: f32,
        screen_h: f32,
    ) {
        // Update screen-size uniform
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&[HudUniform {
                screen_size: [screen_w, screen_h],
                px_range: self.font.px_range,
                _pad: 0.0,
            }]),
        );

        let mut verts: Vec<HudVertex> = Vec::with_capacity(512);
        let font = &self.font;

        // --- Plant readout (top-left) ---
        // The FloraForge spruce sits top-left as a translucent brand mark; the population
        // count nestles inside its lower canopy, then a red/green weekly delta below the
        // tree. Drop shadows keep the digits legible over the green and the scene.
        let margin = 18.0;

        let hero_px = 28.0;
        let hero = format_grouped_count(plant_count as i64);
        let hero_w = font.measure(&hero, hero_px);

        // Tree sized so its lower canopy is wide enough to seat the number (the silhouette
        // is ~0.54 of the image width, so 2.5× the digit width clears it comfortably).
        let bw = hero_w * 2.5;
        let bh = bw / self.backdrop_aspect;
        push_image_quad(&mut verts, margin, margin, bw, bh, [1.0, 1.0, 1.0, 0.92]);

        // Count centered horizontally; seated low in the lower canopy near the base.
        let hero_x = margin + (bw - hero_w) * 0.5;
        let hero_y = margin + bh * 0.82 - font.ascender * hero_px * 0.5;
        push_text_shadowed(
            font,
            &hero,
            hero_x,
            hero_y,
            hero_px,
            [1.0, 1.0, 1.0, 0.98],
            &mut verts,
        );

        // Weekly delta: green for a net gain, red for a net loss, led by an arrow.
        // Seated just above the count, in the open green of the lower canopy, so it doesn't
        // crowd the spruce's base line.
        let delta_px = hero_px * 0.42;
        let delta_y = hero_y - delta_px * 1.4;
        let gain = weekly_delta >= 0;
        let delta_color = if gain {
            [0.49, 0.89, 0.55, 0.95]
        } else {
            [0.95, 0.42, 0.40, 0.95]
        };
        let arrow_w = delta_px * 0.46;
        let arrow_h = delta_px * 0.5;
        let mag = format_grouped_count(weekly_delta);
        let mag_w = font.measure(&mag, delta_px);
        let suffix = " / week";
        let delta_w = arrow_w + 7.0 + mag_w + font.measure(suffix, delta_px);
        let delta_x = margin + (bw - delta_w) * 0.5;
        let arrow_cy = delta_y + font.ascender * delta_px * 0.6;
        push_delta_arrow(
            &mut verts,
            delta_x + arrow_w * 0.5,
            arrow_cy,
            arrow_w,
            arrow_h,
            gain,
            delta_color,
        );
        let mag_x = delta_x + arrow_w + 7.0;
        push_text_shadowed(
            font,
            &mag,
            mag_x,
            delta_y,
            delta_px,
            delta_color,
            &mut verts,
        );
        push_text_shadowed(
            font,
            suffix,
            mag_x + mag_w,
            delta_y,
            delta_px,
            [1.0, 1.0, 1.0, 0.6],
            &mut verts,
        );

        // --- FPS (bottom-right), kept understated and clear of the bottom-left minimap ---
        let fps_px = 13.0;
        let fps_str = format!("{fps:.0} fps");
        push_text_shadowed(
            font,
            &fps_str,
            screen_w - margin - font.measure(&fps_str, fps_px),
            screen_h - margin - font.ascender * fps_px,
            fps_px,
            [1.0, 1.0, 1.0, 0.45],
            &mut verts,
        );

        // --- Compass rose (top-right) ---
        build_compass(font, &mut verts, camera_yaw, screen_w);

        // --- Sky clock + day counter (below compass) ---
        build_sky_clock(font, &mut verts, hour, day_number, screen_w);

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

// SDF shape ids carried in `HudVertex.sdf[0]`; the shader reconstructs each shape from
// the vertex's normalized local coordinate (in `uv`) and feathers the edge to ~1px.
// `sdf[0] == 0` ([`SDF_PLAIN`]) means an ordinary solid/text vertex. Must match `hud.wgsl`.
/// Filled rhombus, boundary `|x| + |y| = 1`.
const SDF_DIAMOND: f32 = 1.0;
/// Filled rhombus that also fades in across the E–W seam (`y = 0`), so an overlaid
/// half cross-fades against the layer below instead of stepping. `sdf[1]` unused.
const SDF_DIAMOND_SEAM: f32 = 2.0;
/// Filled disc, boundary `|p| = 1`.
const SDF_DISC: f32 = 3.0;
/// Annulus: kept where `inner <= |p| <= 1`, with `sdf[1]` the inner radius (normalized).
const SDF_RING: f32 = 4.0;
/// Filled box, boundary `max(|x|, |y|) = 1` (used for the thin sun-ray spokes).
const SDF_BOX: f32 = 5.0;
/// Textured quad sampling `image_texture` at `uv` (0..1), tinted/faded by the vertex
/// color. Used for the plant-count backdrop photo.
const SDF_IMAGE: f32 = 6.0;

fn push_tri(verts: &mut Vec<HudVertex>, a: [f32; 2], b: [f32; 2], c: [f32; 2], color: [f32; 4]) {
    verts.push(HudVertex {
        position: a,
        uv: NO_UV,
        color,
        sdf: SDF_PLAIN,
    });
    verts.push(HudVertex {
        position: b,
        uv: NO_UV,
        color,
        sdf: SDF_PLAIN,
    });
    verts.push(HudVertex {
        position: c,
        uv: NO_UV,
        color,
        sdf: SDF_PLAIN,
    });
}

/// Appends an anti-aliased SDF primitive as a rotated screen-space quad.
///
/// The quad is centered at `(cx, cy)` and oriented by `(cos_t, sin_t)` (its local
/// x-axis maps to `(cos_t, sin_t)`). `hw`/`hh` are the shape's half-extents in pixels
/// along its local x/y axes; `margin` pixels of headroom are added on every side so the
/// fragment shader has room to feather the edge. Each vertex carries its normalized
/// local coordinate in `uv` (shape edge at `|coord| = 1`) and `[shape, param]` in `sdf`,
/// from which `hud.wgsl` evaluates the matching signed distance field.
#[allow(clippy::too_many_arguments)]
fn push_sdf_quad(
    verts: &mut Vec<HudVertex>,
    cx: f32,
    cy: f32,
    cos_t: f32,
    sin_t: f32,
    hw: f32,
    hh: f32,
    margin: f32,
    shape: f32,
    param: f32,
    color: [f32; 4],
) {
    let ext_x = 1.0 + margin / hw;
    let ext_y = 1.0 + margin / hh;
    let corner = |nx: f32, ny: f32| {
        let sx = nx * hw;
        let sy = ny * hh;
        HudVertex {
            position: [cx + sx * cos_t - sy * sin_t, cy + sx * sin_t + sy * cos_t],
            uv: [nx, ny],
            color,
            sdf: [shape, param],
        }
    };
    let tl = corner(-ext_x, -ext_y);
    let tr = corner(ext_x, -ext_y);
    let br = corner(ext_x, ext_y);
    let bl = corner(-ext_x, ext_y);
    verts.extend_from_slice(&[tl, tr, br, tl, br, bl]);
}

/// Convenience for a non-rotated, axis-aligned circular SDF primitive (disc/ring)
/// of radius `r` centered at `(cx, cy)`.
fn push_sdf_circle(
    verts: &mut Vec<HudVertex>,
    cx: f32,
    cy: f32,
    r: f32,
    shape: f32,
    param: f32,
    color: [f32; 4],
) {
    push_sdf_quad(verts, cx, cy, 1.0, 0.0, r, r, 1.5, shape, param, color);
}

/// Formats a non-negative integer with thousands separators, e.g. `48210 → "48,210"`.
/// Negative inputs are formatted by magnitude (the caller carries the sign separately,
/// e.g. via the delta arrow's direction).
fn format_grouped_count(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, &b) in bytes.iter().enumerate() {
        // Insert a separator before every group of three counted from the right.
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

/// Pushes an axis-aligned textured quad sampling `image_texture` across `0..1` uv, with
/// `tint` multiplying the sampled texels (its alpha fades the whole image). `(x, y)` is
/// the top-left corner in screen pixels.
fn push_image_quad(verts: &mut Vec<HudVertex>, x: f32, y: f32, w: f32, h: f32, tint: [f32; 4]) {
    let v = |px: f32, py: f32, u: f32, vv: f32| HudVertex {
        position: [px, py],
        uv: [u, vv],
        color: tint,
        sdf: [SDF_IMAGE, 0.0],
    };
    let (tl, tr, br, bl) = (
        v(x, y, 0.0, 0.0),
        v(x + w, y, 1.0, 0.0),
        v(x + w, y + h, 1.0, 1.0),
        v(x, y + h, 0.0, 1.0),
    );
    verts.extend_from_slice(&[tl, tr, br, tl, br, bl]);
}

/// A small filled triangle marking the sign of the weekly delta: apex up for a gain,
/// apex down for a loss. Centered at `(cx, cy)` with base width `w` and height `h`.
fn push_delta_arrow(
    verts: &mut Vec<HudVertex>,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    up: bool,
    color: [f32; 4],
) {
    let (hw, hh) = (w * 0.5, h * 0.5);
    if up {
        push_tri(
            verts,
            [cx, cy - hh],
            [cx + hw, cy + hh],
            [cx - hw, cy + hh],
            color,
        );
    } else {
        push_tri(
            verts,
            [cx, cy + hh],
            [cx + hw, cy - hh],
            [cx - hw, cy - hh],
            color,
        );
    }
}

/// Draws `text` with a soft drop shadow so light type stays legible over any scene:
/// a dark copy is laid down first, offset down-right by a size-proportional amount,
/// then the foreground copy on top.
fn push_text_shadowed(
    font: &MsdfFont,
    text: &str,
    x: f32,
    y: f32,
    px: f32,
    color: [f32; 4],
    verts: &mut Vec<HudVertex>,
) {
    let off = (px * 0.045).max(1.5);
    let shadow = [0.0, 0.0, 0.0, color[3] * 0.6];
    hud_font::build_text_quads(font, text, x + off, y + off, px, shadow, verts);
    hud_font::build_text_quads(font, text, x, y, px, color, verts);
}

fn build_compass(font: &MsdfFont, verts: &mut Vec<HudVertex>, yaw: f32, screen_w: f32) {
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

    let north_color = [0.9, 0.15, 0.1, 0.9]; // bright red
    let south_color = [0.35, 0.1, 0.08, 0.9]; // dark

    // The diamond is two overlaid SDF quads (red, then brown on top), each feathering
    // the rhombus edge to ~1px in the shader — replacing a hard-edged triangle fan that
    // aliased against the scene. The brown layer fades in across the E–W seam (local
    // y = 0) so the red/brown divider is anti-aliased too, cross-fading over the red
    // layer rather than letting the background bleed through. Local x maps to screen via
    // (cos_y, sin_y), so the diamond's local +x is the east point and -y is north.
    let margin = 1.5;
    push_sdf_quad(
        verts,
        cx,
        cy,
        cos_y,
        sin_y,
        radius,
        radius,
        margin,
        SDF_DIAMOND,
        0.0,
        north_color,
    );
    push_sdf_quad(
        verts,
        cx,
        cy,
        cos_y,
        sin_y,
        radius,
        radius,
        margin,
        SDF_DIAMOND_SEAM,
        0.0,
        south_color,
    );

    // Cardinal labels
    let label_color = [1.0, 1.0, 1.0, 0.95];
    let px = COMPASS_LABEL_PX;
    // Approximate cap height of the uppercase labels, in em — used to center each
    // glyph vertically on its compass point (build_text_quads positions by top-left).
    const CAP_EM: f32 = 0.72;

    for (angle, label) in [
        (0.0, "N"),
        (std::f32::consts::FRAC_PI_2, "E"),
        (std::f32::consts::PI, "S"),
        (3.0 * std::f32::consts::FRAC_PI_2, "W"),
    ] {
        let pos = rot(angle, label_radius);
        let lx = pos[0] - font.measure(label, px) * 0.5;
        // Place the line top so the cap's midline lands on `pos.y`.
        let ly = pos[1] + (0.5 * CAP_EM - font.ascender) * px;
        hud_font::build_text_quads(font, label, lx, ly, px, label_color, verts);
    }
}

/// Sky clock: a 24-hour dial showing sun/moon position relative to the horizon.
/// Placed below the compass rose in the top-right corner.
///
/// Layout:  12h (noon) at top, 0h (midnight) at bottom,
///          6h (sunrise) at left, 18h (sunset) at right.
fn build_sky_clock(
    font: &MsdfFont,
    verts: &mut Vec<HudVertex>,
    hour: f32,
    day_number: u64,
    screen_w: f32,
) {
    use std::f32::consts::{PI, TAU};

    let cx = screen_w - 70.0;
    let cy = 150.0;
    let radius = 26.0;
    let ring_w = 1.5;

    // --- Dark background disc for contrast ---
    let bg_r = radius + 6.0;
    let bg_color = [0.0, 0.0, 0.0, 0.35];
    push_sdf_circle(verts, cx, cy, bg_r, SDF_DISC, 0.0, bg_color);

    // --- Circle ring --- (annulus normalized to its outer radius; the shader keeps the
    // band between the inner ratio and 1.0 and anti-aliases both edges)
    let inner_r = radius - ring_w;
    let outer_r = radius + ring_w;
    let ring_color = [0.8, 0.8, 0.8, 0.6];
    push_sdf_circle(
        verts,
        cx,
        cy,
        outer_r,
        SDF_RING,
        inner_r / outer_r,
        ring_color,
    );

    // --- Horizon line ---
    let horizon_color = [0.6, 0.6, 0.6, 0.35];
    let hw = radius - 3.0;
    let hh = 0.75;
    push_tri(
        verts,
        [cx - hw, cy - hh],
        [cx + hw, cy - hh],
        [cx + hw, cy + hh],
        horizon_color,
    );
    push_tri(
        verts,
        [cx - hw, cy - hh],
        [cx + hw, cy + hh],
        [cx - hw, cy + hh],
        horizon_color,
    );

    // --- Tick marks at 0h, 6h, 12h, 18h ---
    let tick_color = [0.8, 0.8, 0.8, 0.45];
    let tick_inner = radius - 4.0;
    let tick_outer = radius + 1.0;
    let tick_half_w = 0.75;

    for &h in &[0.0_f32, 6.0, 12.0, 18.0] {
        let a = (h - 12.0) / 12.0 * PI;
        let dx = a.sin();
        let dy = -a.cos();
        // Perpendicular
        let px = -dy * tick_half_w;
        let py = dx * tick_half_w;

        let p0 = [cx + dx * tick_inner + px, cy + dy * tick_inner + py];
        let p1 = [cx + dx * tick_inner - px, cy + dy * tick_inner - py];
        let p2 = [cx + dx * tick_outer + px, cy + dy * tick_outer + py];
        let p3 = [cx + dx * tick_outer - px, cy + dy * tick_outer - py];

        push_tri(verts, p0, p2, p3, tick_color);
        push_tri(verts, p0, p3, p1, tick_color);
    }

    // --- Sun / Moon body ---
    let angle = (hour - 12.0) / 12.0 * PI;
    let bx = cx + angle.sin() * radius;
    let by = cy - angle.cos() * radius;

    // Above horizon → sun (gold), below → moon (pale blue)
    let is_sun = by < cy - 0.5;
    let is_moon = by > cy + 0.5;

    let body_color = if is_sun {
        [1.0, 0.85, 0.2, 0.95]
    } else if is_moon {
        [0.8, 0.85, 1.0, 0.85]
    } else {
        // Near horizon — blend toward orange
        [1.0, 0.6, 0.2, 0.9]
    };

    let body_r = 6.0;
    push_sdf_circle(verts, bx, by, body_r, SDF_DISC, 0.0, body_color);

    // Sun rays (only when above horizon): thin radial spokes, each an anti-aliased SDF
    // box. The box's local x-axis is set perpendicular to the spoke so its half-extents
    // are (width, length); it's centered at the spoke's midpoint out from the sun body.
    if is_sun {
        let ray_color = [1.0, 0.9, 0.3, 0.5];
        let ray_inner = body_r + 1.5;
        let ray_outer = body_r + 4.5;
        let ray_half_w = 0.6;
        let ray_half_len = (ray_outer - ray_inner) * 0.5;
        let ray_mid = (ray_inner + ray_outer) * 0.5;
        let ray_count = 8;
        for i in 0..ray_count {
            let a = (i as f32 / ray_count as f32) * TAU;
            let (rdy_dir, rdx_dir) = (a.sin(), a.cos());
            // Spoke direction (rdx_dir, rdy_dir); local x-axis = perpendicular to it.
            let (perp_x, perp_y) = (-rdy_dir, rdx_dir);
            let mx = bx + rdx_dir * ray_mid;
            let my = by + rdy_dir * ray_mid;
            push_sdf_quad(
                verts,
                mx,
                my,
                perp_x,
                perp_y,
                ray_half_w,
                ray_half_len,
                1.0,
                SDF_BOX,
                0.0,
                ray_color,
            );
        }
    }

    // --- Day counter, captioned beneath the dial so it reads as part of it ---
    let day_px = 14.0;
    let day_str = format!("Day {day_number}");
    let day_x = cx - font.measure(&day_str, day_px) * 0.5;
    let day_y = cy + bg_r + 5.0;
    push_text_shadowed(
        font,
        &day_str,
        day_x,
        day_y,
        day_px,
        [1.0, 1.0, 1.0, 0.9],
        verts,
    );
}

#[cfg(test)]
mod tests {
    use super::format_grouped_count;

    #[test]
    fn grouped_count_inserts_thousands_separators() {
        assert_eq!(format_grouped_count(0), "0");
        assert_eq!(format_grouped_count(42), "42");
        assert_eq!(format_grouped_count(999), "999");
        assert_eq!(format_grouped_count(1_000), "1,000");
        assert_eq!(format_grouped_count(48_210), "48,210");
        assert_eq!(format_grouped_count(14_165_889), "14,165,889");
    }

    #[test]
    fn grouped_count_formats_magnitude_of_negatives() {
        // The weekly-delta caller passes the magnitude and shows sign via the arrow.
        assert_eq!(format_grouped_count(-860), "860");
    }
}
