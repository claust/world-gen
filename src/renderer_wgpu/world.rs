use std::collections::HashMap;

use glam::{IVec2, Mat4, Vec3};

use super::frustum::Frustum;
use super::hud_pass::HudPass;
use super::instanced_pass::InstancedPass;
use super::instancing::{GpuInstanceChunk, PrototypeMesh};
use super::material::{FrameBindGroup, FrameUniform, MaterialBindGroup};
use super::minimap_pass::MinimapPass;
use super::sky::SkyPalette;
use super::sky_pass::SkyPass;
use super::terrain_pass::TerrainPass;
use super::terrain_texture::TerrainTexture;
use super::water_pass::WaterPass;
use crate::renderer_wgpu::pipeline::DepthTexture;
use crate::world_core::chunk::ChunkData;
use crate::world_core::herbarium::PlantRegistry;

pub struct WorldRenderer {
    frame_bg: FrameBindGroup,
    terrain_material: MaterialBindGroup,
    terrain_texture: TerrainTexture,
    depth: DepthTexture,
    sky: SkyPass,
    terrain: TerrainPass,
    water: WaterPass,
    instanced: InstancedPass,
    hud: HudPass,
    minimap: MinimapPass,
    fog_color: [f32; 3],
    fog_start: f32,
    fog_end: f32,
    registry: PlantRegistry,
    view_proj: Mat4,
    camera_position: Vec3,
}

impl WorldRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &wgpu::SurfaceConfiguration,
        render_format: wgpu::TextureFormat,
        sea_level: f32,
        load_radius: i32,
        registry: PlantRegistry,
    ) -> Self {
        let frame_bg = FrameBindGroup::new(device);
        let terrain_material = MaterialBindGroup::new_terrain(device);
        let terrain_texture = TerrainTexture::new(device, queue);

        // Shared 2-group layout for sky/water/instanced passes
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shared-pipeline-layout"),
            bind_group_layouts: &[&frame_bg.layout, &terrain_material.layout],
            push_constant_ranges: &[],
        });

        let sky = SkyPass::new(device, render_format, &pipeline_layout);
        // Terrain gets its own 3-group layout with the texture atlas
        let terrain = TerrainPass::new(
            device,
            render_format,
            &frame_bg.layout,
            &terrain_material.layout,
            &terrain_texture.bind_group_layout,
        );
        let water = WaterPass::new(device, render_format, &pipeline_layout, sea_level);
        let instanced = InstancedPass::new(device, render_format, &pipeline_layout, &registry);
        let hud = HudPass::new(device, queue, render_format);
        let minimap = MinimapPass::new(device, queue, render_format);

        let r = load_radius as f32;
        let fog_start = r * 256.0 * 0.6;
        let fog_end = (r + 0.5) * 256.0;
        let fog_color = [0.45, 0.68, 0.96];

        Self {
            frame_bg,
            terrain_material,
            terrain_texture,
            depth: DepthTexture::new(device, config, "terrain-depth"),
            sky,
            terrain,
            water,
            instanced,
            hud,
            minimap,
            fog_color,
            fog_start,
            fog_end,
            registry,
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
        }
    }

    /// Rebuild species prototype meshes and clear instance caches for an updated registry.
    pub fn update_registry(&mut self, device: &wgpu::Device, registry: PlantRegistry) {
        self.instanced.rebuild_species(device, &registry);
        self.registry = registry;
    }

    pub fn set_sea_level(&mut self, _queue: &wgpu::Queue, sea_level: f32) {
        self.water.set_sea_level(sea_level);
    }

    pub fn set_load_radius(&mut self, load_radius: i32) {
        let r = load_radius as f32;
        self.fog_start = r * 256.0 * 0.6;
        self.fog_end = (r + 0.5) * 256.0;
    }

    pub fn resize(&mut self, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) {
        self.depth = DepthTexture::new(device, config, "terrain-depth");
    }

    pub fn update_frame(
        &mut self,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        camera_position: Vec3,
        elapsed: f32,
        hour: f32,
    ) {
        self.view_proj = view_proj;
        self.camera_position = camera_position;
        self.frame_bg.update(
            queue,
            &FrameUniform::new(view_proj, camera_position, elapsed, hour),
        );
    }

    pub fn update_material(
        &mut self,
        queue: &wgpu::Queue,
        light_direction: Vec3,
        ambient: f32,
        palette: &SkyPalette,
    ) {
        self.fog_color = palette.horizon;
        self.terrain_material.update_terrain(
            queue,
            light_direction,
            ambient,
            [self.fog_start, self.fog_end, 0.0, 0.0],
            palette,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_hud(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        camera_pos: Vec3,
        camera_yaw: f32,
        hour: f32,
        screen_w: f32,
        screen_h: f32,
    ) {
        self.hud.update(
            queue, device, camera_pos, camera_yaw, hour, screen_w, screen_h,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_minimap(
        &mut self,
        queue: &wgpu::Queue,
        device: &wgpu::Device,
        dt: f32,
        camera_pos: Vec3,
        camera_yaw: f32,
        camera_fov: f32,
        screen_w: f32,
        screen_h: f32,
    ) {
        self.minimap.update(
            queue, device, dt, camera_pos, camera_yaw, camera_fov, screen_w, screen_h,
        );
    }

    pub fn clear_chunks(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let empty = HashMap::new();
        self.sync_chunks(device, queue, &empty);
    }

    pub fn sync_chunks(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        chunks: &HashMap<IVec2, ChunkData>,
    ) {
        self.sync_terrain(device, queue, chunks);
        self.sync_water(device, chunks);
        self.sync_instances(device, chunks);
        self.sync_minimap(queue, chunks);
    }

    pub fn sync_terrain(
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
    }

    pub fn sync_water(&mut self, device: &wgpu::Device, chunks: &HashMap<IVec2, ChunkData>) {
        self.water.sync_chunks(device, chunks);
    }

    pub fn sync_instances(&mut self, device: &wgpu::Device, chunks: &HashMap<IVec2, ChunkData>) {
        self.instanced.sync_chunks(device, chunks, &self.registry);
    }

    pub fn sync_minimap(&mut self, queue: &wgpu::Queue, chunks: &HashMap<IVec2, ChunkData>) {
        self.minimap.sync_chunks(queue, chunks);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn apply_model_reloads(&mut self, device: &wgpu::Device, reloads: &[(String, Vec<u8>)]) {
        self.instanced.apply_model_reloads(device, reloads);
    }

    /// Render sky + custom meshes for the plant editor.
    pub fn render_editor_scene<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        meshes: &[(&'a PrototypeMesh, &'a GpuInstanceChunk)],
    ) {
        pass.set_bind_group(0, &self.frame_bg.bind_group, &[]);
        pass.set_bind_group(1, &self.terrain_material.bind_group, &[]);
        self.sky.render(pass);
        self.instanced.render_custom(pass, meshes);
    }

    /// Render only the sky pass (used for menu background).
    pub fn render_sky_only<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_bind_group(0, &self.frame_bg.bind_group, &[]);
        pass.set_bind_group(1, &self.terrain_material.bind_group, &[]);
        self.sky.render(pass);
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        self.render_scene(pass);
        self.hud.render(pass);
        self.minimap.render(pass);
    }

    /// Render the 3D scene without HUD/minimap overlays.
    pub fn render_scene<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_bind_group(0, &self.frame_bg.bind_group, &[]);
        pass.set_bind_group(1, &self.terrain_material.bind_group, &[]);

        let frustum = Frustum::from_view_proj(self.view_proj);
        self.sky.render(pass);
        self.terrain
            .render(pass, &frustum, &self.terrain_texture.bind_group);
        self.instanced.render(pass, &frustum, self.camera_position);
        self.water.render(pass, &frustum);
    }

    pub fn clear_color(&self) -> wgpu::Color {
        wgpu::Color {
            r: self.fog_color[0] as f64,
            g: self.fog_color[1] as f64,
            b: self.fog_color[2] as f64,
            a: 1.0,
        }
    }

    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth.view
    }
}
