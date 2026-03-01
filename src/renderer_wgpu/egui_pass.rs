use egui_wgpu::ScreenDescriptor;

pub struct EguiPass {
    renderer: egui_wgpu::Renderer,
}

impl EguiPass {
    pub fn new(device: &wgpu::Device, output_format: wgpu::TextureFormat) -> Self {
        let renderer = egui_wgpu::Renderer::new(
            device,
            output_format,
            egui_wgpu::RendererOptions {
                depth_stencil_format: None,
                msaa_samples: 1,
                dithering: false,
                ..Default::default()
            },
        );
        Self { renderer }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        screen: ScreenDescriptor,
        full_output: egui::FullOutput,
        ctx: &egui::Context,
    ) {
        // Update textures
        for (id, delta) in &full_output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, delta);
        }

        // Tessellate
        let paint_jobs = ctx.tessellate(full_output.shapes, full_output.pixels_per_point);

        // Update vertex/index buffers
        self.renderer
            .update_buffers(device, queue, encoder, &paint_jobs, &screen);

        // Render in a separate pass (LoadOp::Load preserves 3D scene)
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // egui-wgpu expects RenderPass<'static>; forget_lifetime() is safe
            // as the pass is used and dropped within this encoder scope.
            self.renderer
                .render(&mut pass.forget_lifetime(), &paint_jobs, &screen);
        }

        // Free old textures
        for id in &full_output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}
