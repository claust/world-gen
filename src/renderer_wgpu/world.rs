use std::collections::HashMap;

use glam::{IVec2, Mat4, Vec3};

use super::hud_pass::HudPass;
use super::instanced_pass::InstancedPass;
use super::material::{FrameBindGroup, FrameUniform, MaterialBindGroup};
use super::terrain_pass::TerrainPass;
use crate::renderer_wgpu::pipeline::DepthTexture;
use crate::world_core::chunk::ChunkData;

pub struct WorldRenderer {
    frame_bg: FrameBindGroup,
    terrain_material: MaterialBindGroup,
    depth: DepthTexture,
    terrain: TerrainPass,
    instanced: InstancedPass,
    hud: HudPass,
}

impl WorldRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &wgpu::SurfaceConfiguration,
    ) -> Self {
        let frame_bg = FrameBindGroup::new(device);
        let terrain_material = MaterialBindGroup::new_terrain(device);

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shared-pipeline-layout"),
            bind_group_layouts: &[&frame_bg.layout, &terrain_material.layout],
            push_constant_ranges: &[],
        });

        let terrain = TerrainPass::new(device, config, &pipeline_layout);
        let instanced = InstancedPass::new(device, config, &pipeline_layout);
        let hud = HudPass::new(device, queue, config);

        Self {
            frame_bg,
            terrain_material,
            depth: DepthTexture::new(device, config, "terrain-depth"),
            terrain,
            instanced,
            hud,
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

    pub fn update_hud(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        camera_pos: Vec3,
        camera_yaw: f32,
        screen_w: f32,
        screen_h: f32,
    ) {
        self.hud
            .update(queue, device, camera_pos, camera_yaw, screen_w, screen_h);
    }

    pub fn sync_chunks(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunks: &HashMap<IVec2, ChunkData>,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("terrain-gen-encoder"),
        });

        let dispatched = self.terrain.sync_chunks(device, &mut encoder, chunks);
        if dispatched {
            queue.submit(Some(encoder.finish()));
        }

        self.instanced.sync_chunks(device, chunks);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn apply_model_reloads(&mut self, device: &wgpu::Device, reloads: &[(String, Vec<u8>)]) {
        self.instanced.apply_model_reloads(device, reloads);
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_bind_group(0, &self.frame_bg.bind_group, &[]);
        pass.set_bind_group(1, &self.terrain_material.bind_group, &[]);

        self.terrain.render(pass);
        self.instanced.render(pass);
        self.hud.render(pass);
    }

    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth.view
    }
}
