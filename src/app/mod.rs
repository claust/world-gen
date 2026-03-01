use anyhow::Result;
use glam::Vec3;
use wgpu::SurfaceError;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, WindowEvent};
use winit::window::{CursorGrabMode, Window};

use crate::renderer_wgpu::camera::{CameraController, FlyCamera};
use crate::renderer_wgpu::gpu_context::GpuContext;
use crate::renderer_wgpu::world::WorldRenderer;
use crate::world_core::config::GameConfig;
use crate::world_runtime::WorldRuntime;

#[cfg(not(target_arch = "wasm32"))]
use crate::renderer_wgpu::egui_bridge::EguiBridge;
#[cfg(not(target_arch = "wasm32"))]
use crate::renderer_wgpu::egui_pass::EguiPass;
#[cfg(not(target_arch = "wasm32"))]
use crate::ui::ConfigPanel;
#[cfg(not(target_arch = "wasm32"))]
use crate::world_core::save::{CameraSave, SaveData, WorldSave};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::debug_api::{start_debug_api, DebugApiConfig, DebugApiHandle};
#[cfg(not(target_arch = "wasm32"))]
use crate::renderer_wgpu::asset_watcher::AssetWatcher;

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

#[cfg(not(target_arch = "wasm32"))]
mod debug_commands;
mod event_loop;
#[cfg(not(target_arch = "wasm32"))]
mod screenshot;

pub use event_loop::run_event_loop;
#[cfg(target_arch = "wasm32")]
pub use event_loop::run_event_loop_web;

pub struct AppState {
    window: &'static Window,
    gpu: GpuContext,
    world_renderer: WorldRenderer,
    camera: FlyCamera,
    camera_controller: CameraController,
    world: WorldRuntime,
    focused: bool,
    cursor_captured: bool,
    last_frame: Instant,
    frame_time_ms: f32,
    elapsed_seconds: f32,
    frame_index: u64,
    #[cfg(not(target_arch = "wasm32"))]
    debug_api: Option<DebugApiHandle>,
    #[cfg(not(target_arch = "wasm32"))]
    last_telemetry_emit: Instant,
    #[cfg(not(target_arch = "wasm32"))]
    screenshot_pending: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    asset_watcher: Option<AssetWatcher>,
    #[cfg(not(target_arch = "wasm32"))]
    egui_bridge: EguiBridge,
    #[cfg(not(target_arch = "wasm32"))]
    egui_pass: EguiPass,
    #[cfg(not(target_arch = "wasm32"))]
    config_panel: ConfigPanel,
}

impl AppState {
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new(
        window: &'static Window,
        debug_api_config: DebugApiConfig,
        cursor_captured: bool,
    ) -> Result<Self> {
        let config = GameConfig::load();
        let save = SaveData::load();

        let gpu = GpuContext::new(window).await?;

        let mut world_renderer = WorldRenderer::new(
            &gpu.device,
            &gpu.queue,
            &gpu.config,
            config.sea_level,
            config.world.load_radius,
        );

        let (cam_pos, cam_yaw, cam_pitch) = match &save {
            Some(s) => (
                Vec3::new(
                    s.camera.position[0],
                    s.camera.position[1],
                    s.camera.position[2],
                ),
                s.camera.yaw,
                s.camera.pitch,
            ),
            None => (Vec3::new(96.0, 150.0, 16.0), 1.02, -0.38),
        };
        let mut camera = FlyCamera::new(cam_pos);
        camera.yaw = cam_yaw;
        camera.pitch = cam_pitch;
        let camera_controller = CameraController::new(180.0, 0.0022);

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let mut world = WorldRuntime::new(&config, save.as_ref(), threads)?;
        world.update(0.0, camera.position);

        world_renderer.sync_chunks(&gpu.device, &gpu.queue, world.chunks());

        let debug_api = start_debug_api(&debug_api_config)?;
        if let Some(api) = &debug_api {
            log::info!("debug api listening on {}", api.bind_addr());
        }

        let asset_watcher = AssetWatcher::start();

        let scale_factor = window.scale_factor() as f32;
        let egui_bridge = EguiBridge::new(scale_factor, gpu.config.width, gpu.config.height);
        let egui_pass = EguiPass::new(&gpu.device, gpu.config.format);
        let config_panel = ConfigPanel::new(&config);

        Ok(Self {
            window,
            gpu,
            world_renderer,
            camera,
            camera_controller,
            world,
            debug_api,
            focused: true,
            cursor_captured,
            last_frame: Instant::now(),
            last_telemetry_emit: Instant::now() - Duration::from_secs(1),
            frame_time_ms: 0.0,
            elapsed_seconds: 0.0,
            frame_index: 0,
            screenshot_pending: None,
            asset_watcher,
            egui_bridge,
            egui_pass,
            config_panel,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new_web(window: &'static Window, cursor_captured: bool) -> Result<Self> {
        let config = GameConfig::default();

        let gpu = GpuContext::new(window).await?;

        let mut world_renderer = WorldRenderer::new(
            &gpu.device,
            &gpu.queue,
            &gpu.config,
            config.sea_level,
            config.world.load_radius,
        );

        let mut camera = FlyCamera::new(Vec3::new(96.0, 150.0, 16.0));
        camera.yaw = 1.02;
        camera.pitch = -0.38;
        let camera_controller = CameraController::new(180.0, 0.0022);

        let mut world = WorldRuntime::new(&config, None, 1)?;
        world.update(0.0, camera.position);

        world_renderer.sync_chunks(&gpu.device, &gpu.queue, world.chunks());

        Ok(Self {
            window,
            gpu,
            world_renderer,
            camera,
            camera_controller,
            world,
            focused: true,
            cursor_captured,
            last_frame: Instant::now(),
            frame_time_ms: 0.0,
            elapsed_seconds: 0.0,
            frame_index: 0,
        })
    }

    fn process_window_event(&mut self, event: &WindowEvent) {
        let _ = self.camera_controller.process_window_event(event);

        if let WindowEvent::Focused(focused) = event {
            self.focused = *focused;
            if !focused {
                self.release_cursor();
            }
        }
    }

    fn process_device_event(&mut self, event: &DeviceEvent) {
        if !(self.focused && self.cursor_captured) {
            return;
        }
        self.camera_controller.process_device_event(event);
    }

    fn capture_cursor(&mut self) {
        self.cursor_captured = try_grab_window_cursor(self.window);
        if self.cursor_captured {
            self.camera_controller.reset_inputs();
        }
    }

    fn release_cursor(&mut self) {
        if !self.cursor_captured {
            return;
        }

        release_window_cursor(self.window);
        self.cursor_captured = false;
        self.camera_controller.reset_inputs();
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.gpu.resize(new_size);
        self.world_renderer
            .resize(&self.gpu.device, &self.gpu.config);
        #[cfg(not(target_arch = "wasm32"))]
        self.egui_bridge
            .resize(self.gpu.config.width, self.gpu.config.height);
    }

    fn update(&mut self) {
        self.frame_index = self.frame_index.saturating_add(1);

        #[cfg(not(target_arch = "wasm32"))]
        self.apply_debug_commands();

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(watcher) = &self.asset_watcher {
            let reloads: Vec<(String, Vec<u8>)> = watcher
                .drain_reloads()
                .into_iter()
                .map(|r| (r.name, r.bytes))
                .collect();
            if !reloads.is_empty() {
                self.world_renderer
                    .apply_model_reloads(&self.gpu.device, &reloads);
            }
        }

        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.frame_time_ms = self.frame_time_ms * 0.94 + (dt * 1000.0) * 0.06;
        self.elapsed_seconds += dt;

        self.camera_controller.update_camera(
            dt,
            &mut self.camera,
            self.focused && self.cursor_captured,
        );

        clamp_camera_to_terrain(&mut self.camera, self.world.chunks());

        self.world.update(dt, self.camera.position);
        self.world_renderer
            .sync_chunks(&self.gpu.device, &self.gpu.queue, self.world.chunks());

        let aspect = self.gpu.aspect();
        let view_proj = self.camera.view_projection(aspect);
        let lighting = self.world.lighting();
        let stats = self.world.stats();
        self.world_renderer.update_frame(
            &self.gpu.queue,
            view_proj,
            self.camera.position,
            self.elapsed_seconds,
            stats.hour,
        );
        self.world_renderer.update_material(
            &self.gpu.queue,
            lighting.sun_direction,
            lighting.ambient,
        );
        self.world_renderer.update_hud(
            &self.gpu.queue,
            &self.gpu.device,
            self.camera.position,
            self.camera.yaw,
            self.gpu.config.width as f32,
            self.gpu.config.height as f32,
        );
        self.world_renderer.update_minimap(
            &self.gpu.queue,
            &self.gpu.device,
            self.camera.position,
            self.camera.yaw,
            self.camera.fov_y_radians,
            self.gpu.config.width as f32,
            self.gpu.config.height as f32,
        );

        // Apply config panel changes (debounced — only on pointer release)
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(new_config) = self.config_panel.take_dirty_config(self.egui_bridge.ctx()) {
            self.world.reload_config(&new_config);
            self.world_renderer
                .set_sea_level(&self.gpu.queue, new_config.sea_level);
            self.world_renderer
                .set_load_radius(new_config.world.load_radius);
            let _ = self.world.set_day_speed(new_config.world.day_speed);
        }

        #[cfg(not(target_arch = "wasm32"))]
        self.publish_telemetry_if_due(&stats);

        self.window.set_title(&format!(
            "world-gen | {:.1}ms ({:.0}fps) | chunks: {}/{} | center: {},{} | hour: {:.1} | day_speed: {:.2}",
            self.frame_time_ms,
            1000.0 / self.frame_time_ms.max(0.01),
            stats.loaded_chunks,
            stats.loaded_chunks + stats.pending_chunks,
            stats.center_chunk.x,
            stats.center_chunk.y,
            stats.hour,
            self.world.day_speed(),
        ));
    }

    fn render(&mut self) -> Result<(), SurfaceError> {
        let output = self.gpu.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("world-gen-render-encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(crate::renderer_wgpu::sky::clear_color()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.world_renderer.depth_view(),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            self.world_renderer.render(&mut pass);
        }

        // egui overlay pass (renders on top of 3D scene)
        #[cfg(not(target_arch = "wasm32"))]
        if self.config_panel.is_visible() {
            let raw_input = self.egui_bridge.take_raw_input();
            let full_output = self.egui_bridge.ctx().run(raw_input, |ctx| {
                self.config_panel.ui(ctx);
            });

            self.egui_bridge
                .handle_platform_output(self.window, &full_output.platform_output);

            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [self.gpu.config.width, self.gpu.config.height],
                pixels_per_point: self.egui_bridge.pixels_per_point(),
            };

            self.egui_pass.render(
                &self.gpu.device,
                &self.gpu.queue,
                &mut encoder,
                &view,
                screen,
                full_output,
                self.egui_bridge.ctx(),
            );
        }

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(command_id) = self.screenshot_pending.take() {
            self.handle_screenshot(command_id, &output.texture, encoder);
            output.present();
            return Ok(());
        }

        self.gpu.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn save_game(&self) {
        let save = SaveData {
            camera: CameraSave {
                position: [
                    self.camera.position.x,
                    self.camera.position.y,
                    self.camera.position.z,
                ],
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
            },
            world: WorldSave {
                seed: self.world.seed(),
                hour: self.world.hour(),
                day_speed: self.world.day_speed(),
            },
        };
        if let Err(e) = save.save() {
            log::warn!("failed to save game state: {e}");
        }
    }
}

pub fn try_grab_window_cursor(window: &Window) -> bool {
    let grabbed = window
        .set_cursor_grab(CursorGrabMode::Locked)
        .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
        .is_ok();

    window.set_cursor_visible(!grabbed);
    grabbed
}

fn release_window_cursor(window: &Window) {
    let _ = window.set_cursor_grab(CursorGrabMode::None);
    window.set_cursor_visible(true);
}

const MIN_HEIGHT_ABOVE_GROUND: f32 = 2.0;

fn clamp_camera_to_terrain(
    camera: &mut FlyCamera,
    chunks: &std::collections::HashMap<glam::IVec2, crate::world_core::chunk::ChunkData>,
) {
    use crate::world_core::chunk::{CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS};

    let cx = (camera.position.x / CHUNK_SIZE_METERS).floor() as i32;
    let cz = (camera.position.z / CHUNK_SIZE_METERS).floor() as i32;
    let Some(chunk) = chunks.get(&glam::IVec2::new(cx, cz)) else {
        return;
    };

    let local_x = camera.position.x - cx as f32 * CHUNK_SIZE_METERS;
    let local_z = camera.position.z - cz as f32 * CHUNK_SIZE_METERS;

    // Bilinear interpolation of height (same logic as sampling.rs)
    let side = CHUNK_GRID_RESOLUTION;
    let xf = ((local_x / CHUNK_SIZE_METERS) * (side - 1) as f32).clamp(0.0, (side - 1) as f32);
    let zf = ((local_z / CHUNK_SIZE_METERS) * (side - 1) as f32).clamp(0.0, (side - 1) as f32);

    let x0 = xf.floor() as usize;
    let z0 = zf.floor() as usize;
    let x1 = (x0 + 1).min(side - 1);
    let z1 = (z0 + 1).min(side - 1);
    let tx = xf - x0 as f32;
    let tz = zf - z0 as f32;

    let h = &chunk.terrain.heights;
    let h00 = h[z0 * side + x0];
    let h10 = h[z0 * side + x1];
    let h01 = h[z1 * side + x0];
    let h11 = h[z1 * side + x1];

    let hx0 = h00 + (h10 - h00) * tx;
    let hx1 = h01 + (h11 - h01) * tx;
    let terrain_height = hx0 + (hx1 - hx0) * tz;

    let min_y = terrain_height + MIN_HEIGHT_ABOVE_GROUND;
    if camera.position.y < min_y {
        camera.position.y = min_y;
    }
}
