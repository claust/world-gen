use crate::renderer_wgpu::pipeline::create_sky_pipeline;

pub struct SkyPass {
    pipeline: wgpu::RenderPipeline,
}

impl SkyPass {
    pub fn new(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        layout: &wgpu::PipelineLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/sky.wgsl").into()),
        });

        let pipeline = create_sky_pipeline(device, config, layout, &shader, "sky-pipeline");

        Self { pipeline }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.draw(0..3, 0..1);
    }
}
