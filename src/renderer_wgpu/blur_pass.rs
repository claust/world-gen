use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct BlurParams {
    direction: [f32; 2],
    texel_size: [f32; 2],
}

pub struct BlurPass {
    blur_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    h_params: wgpu::Buffer,
    v_params: wgpu::Buffer,
    tex_a: wgpu::Texture,
    view_a: wgpu::TextureView,
    tex_b: wgpu::Texture,
    view_b: wgpu::TextureView,
    bg_a_h: wgpu::BindGroup,
    bg_b_v: wgpu::BindGroup,
    bg_a_blit: wgpu::BindGroup,
    has_result: bool,
    render_format: wgpu::TextureFormat,
    width: u32,
    height: u32,
}

impl BlurPass {
    pub fn new(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blur-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blur.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blur-bind-group-layout"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blur-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let blur_pipeline = Self::create_pipeline(
            device,
            render_format,
            &pipeline_layout,
            &shader,
            "fs_blur",
            "blur-pipeline",
        );
        let blit_pipeline = Self::create_pipeline(
            device,
            render_format,
            &pipeline_layout,
            &shader,
            "fs_blit",
            "blit-pipeline",
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blur-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let w = width.max(1);
        let h = height.max(1);

        let h_params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("blur-h-params"),
            contents: bytemuck::bytes_of(&BlurParams {
                direction: [1.0, 0.0],
                texel_size: [1.0 / w as f32, 1.0 / h as f32],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let v_params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("blur-v-params"),
            contents: bytemuck::bytes_of(&BlurParams {
                direction: [0.0, 1.0],
                texel_size: [1.0 / w as f32, 1.0 / h as f32],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let (tex_a, view_a) = Self::create_texture(device, render_format, w, h, "blur-tex-a", true);
        let (tex_b, view_b) =
            Self::create_texture(device, render_format, w, h, "blur-tex-b", false);

        let bg_a_h = Self::create_bind_group(
            device,
            &bind_group_layout,
            &view_a,
            &sampler,
            &h_params,
            "blur-bg-a-h",
        );
        let bg_b_v = Self::create_bind_group(
            device,
            &bind_group_layout,
            &view_b,
            &sampler,
            &v_params,
            "blur-bg-b-v",
        );
        let bg_a_blit = Self::create_bind_group(
            device,
            &bind_group_layout,
            &view_a,
            &sampler,
            &h_params,
            "blur-bg-a-blit",
        );

        Self {
            blur_pipeline,
            blit_pipeline,
            bind_group_layout,
            sampler,
            h_params,
            v_params,
            tex_a,
            view_a,
            tex_b,
            view_b,
            bg_a_h,
            bg_b_v,
            bg_a_blit,
            has_result: false,
            render_format,
            width: w,
            height: h,
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, width: u32, height: u32) {
        let w = width.max(1);
        let h = height.max(1);
        if w == self.width && h == self.height {
            return;
        }
        self.width = w;
        self.height = h;
        self.has_result = false;

        let (tex_a, view_a) =
            Self::create_texture(device, self.render_format, w, h, "blur-tex-a", true);
        let (tex_b, view_b) =
            Self::create_texture(device, self.render_format, w, h, "blur-tex-b", false);

        let texel_size = [1.0 / w as f32, 1.0 / h as f32];
        queue.write_buffer(
            &self.h_params,
            0,
            bytemuck::bytes_of(&BlurParams {
                direction: [1.0, 0.0],
                texel_size,
            }),
        );
        queue.write_buffer(
            &self.v_params,
            0,
            bytemuck::bytes_of(&BlurParams {
                direction: [0.0, 1.0],
                texel_size,
            }),
        );

        self.bg_a_h = Self::create_bind_group(
            device,
            &self.bind_group_layout,
            &view_a,
            &self.sampler,
            &self.h_params,
            "blur-bg-a-h",
        );
        self.bg_b_v = Self::create_bind_group(
            device,
            &self.bind_group_layout,
            &view_b,
            &self.sampler,
            &self.v_params,
            "blur-bg-b-v",
        );
        self.bg_a_blit = Self::create_bind_group(
            device,
            &self.bind_group_layout,
            &view_a,
            &self.sampler,
            &self.h_params,
            "blur-bg-a-blit",
        );

        self.tex_a = tex_a;
        self.view_a = view_a;
        self.tex_b = tex_b;
        self.view_b = view_b;
    }

    /// Capture the source texture and apply Gaussian blur.
    pub fn capture_and_blur(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        iterations: u32,
    ) {
        let size = wgpu::Extent3d {
            width: self.width,
            height: self.height,
            depth_or_array_layers: 1,
        };

        // Copy source frame into tex_a
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: source,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &self.tex_a,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            size,
        );

        for _ in 0..iterations {
            // Horizontal: sample tex_a → render to tex_b
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("blur-h-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view_b,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.blur_pipeline);
                pass.set_bind_group(0, &self.bg_a_h, &[]);
                pass.draw(0..3, 0..1);
            }

            // Vertical: sample tex_b → render to tex_a
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("blur-v-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.view_a,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.blur_pipeline);
                pass.set_bind_group(0, &self.bg_b_v, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        self.has_result = true;
    }

    /// Blit the stored blurred texture onto the current render pass.
    pub fn blit<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.blit_pipeline);
        pass.set_bind_group(0, &self.bg_a_blit, &[]);
        pass.draw(0..3, 0..1);
    }

    pub fn has_result(&self) -> bool {
        self.has_result
    }

    pub fn clear_result(&mut self) {
        self.has_result = false;
    }

    fn create_pipeline(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
        fs_entry: &str,
        label: &str,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs_entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: render_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        })
    }

    fn create_texture(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        label: &str,
        copy_dst: bool,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let mut usage =
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT;
        if copy_dst {
            usage |= wgpu::TextureUsages::COPY_DST;
        }

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    fn create_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        texture_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        params_buffer: &wgpu::Buffer,
        label: &str,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        })
    }
}

use wgpu::util::DeviceExt;
