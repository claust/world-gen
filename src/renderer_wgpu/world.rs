use std::collections::HashMap;

use glam::{IVec2, Mat4, Vec3};

use super::frustum::Frustum;
use super::hud_pass::HudPass;
use super::instanced_pass::{InstancedPass, InstancedStats};
use super::instancing::{GpuInstanceChunk, PrototypeMesh};
use super::material::{FrameBindGroup, FrameUniform, MaterialBindGroup};
use super::minimap_pass::MinimapPass;
use super::sign_text::SignTextPass;
use super::sky::SkyPalette;
use super::sky_pass::SkyPass;
use super::terrain_pass::TerrainPass;
use super::terrain_texture::TerrainTexture;
use super::water_pass::WaterPass;
use crate::renderer_wgpu::pipeline::{DepthTexture, ShadowMap};
use crate::world_core::chunk::ChunkData;
use crate::world_core::herbarium::PlantRegistry;

pub struct WorldRenderer {
    frame_bg: FrameBindGroup,
    terrain_material: MaterialBindGroup,
    terrain_texture: TerrainTexture,
    depth: DepthTexture,
    shadow_map: ShadowMap,
    sky: SkyPass,
    terrain: TerrainPass,
    water: WaterPass,
    instanced: InstancedPass,
    sign_text: SignTextPass,
    hud: HudPass,
    minimap: MinimapPass,
    fog_color: [f32; 3],
    fog_start: f32,
    fog_end: f32,
    /// Water surface height; used to derive the per-frame underwater submerge
    /// factor uploaded in `FrameUniform.time.z`.
    sea_level: f32,
    registry: PlantRegistry,
    view_proj: Mat4,
    camera_position: Vec3,
    /// Sun light view-projection matrix for the current frame (single cascade).
    light_view_proj: Mat4,
    /// 1.0 when shadows are active (daytime), 0.0 at night.
    shadow_enabled: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RendererStats {
    pub buffered_mature_plants: usize,
    pub buffered_lod_plants: usize,
    pub buffered_house_instances: usize,
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
        let shadow_map = ShadowMap::new(device);

        // Shared 2-group layout for the sky pass (no shadow sampling).
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky-pipeline-layout"),
            bind_group_layouts: &[&frame_bg.layout, &terrain_material.layout],
            push_constant_ranges: &[],
        });

        let sky = SkyPass::new(device, render_format, &sky_layout);
        // Terrain gets its own 4-group layout: frame, material, texture, shadow.
        let terrain = TerrainPass::new(
            device,
            render_format,
            &frame_bg.layout,
            &terrain_material.layout,
            &terrain_texture.bind_group_layout,
            &shadow_map.layout,
        );
        let water = WaterPass::new(
            device,
            render_format,
            &frame_bg.layout,
            &terrain_material.layout,
            &shadow_map.layout,
            sea_level,
        );
        let instanced = InstancedPass::new(
            device,
            render_format,
            &frame_bg.layout,
            &terrain_material.layout,
            &shadow_map.layout,
            &registry,
        );
        let sign_text = SignTextPass::new(device, queue, render_format, &frame_bg.layout);
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
            shadow_map,
            sky,
            terrain,
            water,
            instanced,
            sign_text,
            hud,
            minimap,
            fog_color,
            fog_start,
            fog_end,
            sea_level,
            registry,
            view_proj: Mat4::IDENTITY,
            camera_position: Vec3::ZERO,
            light_view_proj: Mat4::IDENTITY,
            shadow_enabled: 0.0,
        }
    }

    /// Rebuild species prototype meshes and clear instance caches for an updated registry.
    pub fn update_registry(&mut self, device: &wgpu::Device, registry: PlantRegistry) {
        self.instanced.rebuild_species(device, &registry);
        self.registry = registry;
    }

    pub fn set_sea_level(&mut self, _queue: &wgpu::Queue, sea_level: f32) {
        self.sea_level = sea_level;
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
        sun_direction: Vec3,
    ) {
        self.view_proj = view_proj;
        self.camera_position = camera_position;

        let (light_view_proj, shadow_enabled) =
            Self::build_light_view_proj(camera_position, sun_direction);
        self.light_view_proj = light_view_proj;
        self.shadow_enabled = shadow_enabled;

        // Underwater submerge factor: 0 above the surface, ramping to 1 over the
        // ~1.5 m below sea level so crossing the surface fades smoothly.
        const SUBMERGE_RAMP: f32 = 1.5;
        let submerge = ((self.sea_level - camera_position.y) / SUBMERGE_RAMP).clamp(0.0, 1.0);

        self.frame_bg.update(
            queue,
            &FrameUniform::with_shadow(
                view_proj,
                camera_position,
                elapsed,
                hour,
                light_view_proj,
                shadow_enabled,
                submerge,
            ),
        );
    }

    /// Build the single-cascade light view-projection matrix centered on the
    /// camera. `sun_direction` is the direction *toward* the sun (as produced by
    /// `WorldClock::sun_direction`); the light therefore travels along
    /// `-sun_direction`. Returns the matrix and a shadow-enabled flag that is
    /// 0.0 when the sun is at or below the horizon (night).
    fn build_light_view_proj(camera_position: Vec3, sun_direction: Vec3) -> (Mat4, f32) {
        // Direction the sunlight travels (points from sun toward the scene).
        let light_dir = (-sun_direction).normalize_or_zero();

        // Sun above the horizon (sun_direction.y > 0) means it is daytime.
        let shadow_enabled = if sun_direction.y > 0.02 { 1.0 } else { 0.0 };

        let center = camera_position;
        let dist = 400.0_f32;
        let light_pos = center - light_dir * dist;

        // Stable up vector: avoid degeneracy when the light is near-vertical.
        let up = if light_dir.x.abs() < 1e-3 && light_dir.z.abs() < 1e-3 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let light_view = Mat4::look_at_rh(light_pos, center, up);

        // Match the camera's right-handed convention (perspective_rh/look_at_rh).
        let half = 350.0_f32;
        let near = 1.0_f32;
        let far = dist + 800.0_f32;
        let light_proj = Mat4::orthographic_rh(-half, half, -half, half, near, far);

        (light_proj * light_view, shadow_enabled)
    }

    /// Render the scene into the shadow map from the sun's point of view.
    /// Depth-only; must run after `sync_chunks` and after `update_frame` (so the
    /// light matrix is uploaded), but before the main color pass. Skipped at
    /// night to avoid an empty/garbage shadow map being sampled (the shader also
    /// disables sampling via `shadow_params.w`).
    pub fn render_shadows(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shadow-pass"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow_map.view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        if self.shadow_enabled < 0.5 {
            // Still clear the map (above) so a stale frame isn't sampled.
            return;
        }

        pass.set_bind_group(0, &self.frame_bg.bind_group, &[]);
        self.terrain.render_depth(&mut pass);
        self.instanced.render_depth(&mut pass);
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
        fps: f32,
        plant_count: usize,
        screen_w: f32,
        screen_h: f32,
    ) {
        self.hud.update(
            queue,
            device,
            camera_pos,
            camera_yaw,
            hour,
            fps,
            plant_count,
            screen_w,
            screen_h,
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
        self.sign_text.sync_chunks(device, chunks);
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
        pass.set_bind_group(2, &self.shadow_map.bind_group, &[]);
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
        self.terrain.render(
            pass,
            &frustum,
            &self.terrain_texture.bind_group,
            &self.shadow_map.bind_group,
        );
        self.instanced.render(
            pass,
            &frustum,
            self.camera_position,
            &self.shadow_map.bind_group,
        );
        self.water
            .render(pass, &frustum, &self.shadow_map.bind_group);
        // Sign labels render last: this pass rebinds group 1 to the font atlas,
        // so it must come after every pass that relies on group 1 being the
        // terrain-material bind group (terrain/instanced/water).
        self.sign_text.render(pass, &frustum, self.camera_position);
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

    pub fn stats(&self) -> RendererStats {
        let InstancedStats {
            buffered_mature_plants,
            buffered_lod_plants,
            buffered_house_instances,
        } = self.instanced.stats();

        RendererStats {
            buffered_mature_plants,
            buffered_lod_plants,
            buffered_house_instances,
        }
    }
}
