use anyhow::Result;
use glam::Vec3;
use wgpu::SurfaceError;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, Event, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window};

use crate::renderer_wgpu::camera::{CameraController, FlyCamera};
use crate::renderer_wgpu::gpu_context::GpuContext;
use crate::renderer_wgpu::world::WorldRenderer;
use crate::world_core::config::GameConfig;
use crate::world_runtime::WorldRuntime;

#[cfg(not(target_arch = "wasm32"))]
use crate::renderer_wgpu::camera::MoveDirection;
#[cfg(not(target_arch = "wasm32"))]
use crate::world_core::save::{CameraSave, SaveData, WorldSave};
#[cfg(not(target_arch = "wasm32"))]
use crate::world_runtime::RuntimeStats;
#[cfg(not(target_arch = "wasm32"))]
use anyhow::Context;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::debug_api::{
    start_debug_api, CameraSnapshot, ChunkSnapshot, CommandAppliedEvent, CommandKind,
    DebugApiConfig, DebugApiHandle, MoveKey, ObjectKind, TelemetrySnapshot,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::renderer_wgpu::asset_watcher::AssetWatcher;

#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use web_time::Instant;

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

        let mut world_renderer =
            WorldRenderer::new(&gpu.device, &gpu.queue, &gpu.config, config.sea_level);

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
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new_web(window: &'static Window, cursor_captured: bool) -> Result<Self> {
        let config = GameConfig::default();

        let gpu = GpuContext::new(window).await?;

        let mut world_renderer =
            WorldRenderer::new(&gpu.device, &gpu.queue, &gpu.config, config.sea_level);

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
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn apply_debug_commands(&mut self) {
        let Some(api) = &mut self.debug_api else {
            return;
        };

        for command in api.drain_commands() {
            let applied = match command.command {
                CommandKind::SetDaySpeed { value } => match self.world.set_day_speed(value) {
                    Ok(day_speed) => CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: true,
                        message: "day speed set".to_string(),
                        day_speed: Some(day_speed),
                        object_id: None,
                        object_position: None,
                    },
                    Err(message) => CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: false,
                        message,
                        day_speed: Some(self.world.day_speed()),
                        object_id: None,
                        object_position: None,
                    },
                },
                CommandKind::SetMoveKey { key, pressed } => {
                    let direction = match key {
                        MoveKey::W => MoveDirection::Forward,
                        MoveKey::A => MoveDirection::Left,
                        MoveKey::S => MoveDirection::Backward,
                        MoveKey::D => MoveDirection::Right,
                        MoveKey::Up => MoveDirection::Up,
                        MoveKey::Down => MoveDirection::Down,
                    };
                    self.camera_controller.set_remote_move(direction, pressed);

                    CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: true,
                        message: format!(
                            "move key {} {}",
                            key.as_str(),
                            if pressed { "pressed" } else { "released" }
                        ),
                        day_speed: None,
                        object_id: None,
                        object_position: None,
                    }
                }
                CommandKind::SetCameraPosition { x, y, z } => {
                    self.camera.position = glam::Vec3::new(x, y, z);
                    CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: true,
                        message: format!("camera position set to ({:.1}, {:.1}, {:.1})", x, y, z),
                        day_speed: None,
                        object_id: None,
                        object_position: None,
                    }
                }
                CommandKind::SetCameraLook { yaw, pitch } => {
                    self.camera.yaw = yaw;
                    self.camera.pitch = pitch.clamp(-1.54, 1.54);
                    CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: true,
                        message: format!("camera look set to yaw={:.2}, pitch={:.2}", yaw, pitch),
                        day_speed: None,
                        object_id: None,
                        object_position: None,
                    }
                }
                CommandKind::FindNearest { kind } => {
                    let cam_pos = self.camera.position;
                    let mut best: Option<(String, [f32; 3], f32)> = None;

                    for (coord, chunk) in self.world.chunks() {
                        let items: Box<dyn Iterator<Item = (usize, glam::Vec3)>> = match kind {
                            ObjectKind::House => Box::new(
                                chunk
                                    .content
                                    .houses
                                    .iter()
                                    .enumerate()
                                    .map(|(i, h)| (i, h.position)),
                            ),
                            ObjectKind::Tree => Box::new(
                                chunk
                                    .content
                                    .trees
                                    .iter()
                                    .enumerate()
                                    .map(|(i, t)| (i, t.position)),
                            ),
                            ObjectKind::Fern => Box::new(
                                chunk
                                    .content
                                    .ferns
                                    .iter()
                                    .enumerate()
                                    .map(|(i, f)| (i, f.position)),
                            ),
                        };

                        let prefix = match kind {
                            ObjectKind::House => "house",
                            ObjectKind::Tree => "tree",
                            ObjectKind::Fern => "fern",
                        };

                        for (idx, pos) in items {
                            let dist = cam_pos.distance_squared(pos);
                            let is_closer = best.as_ref().is_none_or(|(_, _, d)| dist < *d);
                            if is_closer {
                                let id = format!("{}-{}_{}-{}", prefix, coord.x, coord.y, idx);
                                best = Some((id, [pos.x, pos.y, pos.z], dist));
                            }
                        }
                    }

                    match best {
                        Some((id, pos, _)) => CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: true,
                            message: format!(
                                "nearest {} at ({:.1}, {:.1}, {:.1})",
                                match kind {
                                    ObjectKind::House => "house",
                                    ObjectKind::Tree => "tree",
                                    ObjectKind::Fern => "fern",
                                },
                                pos[0],
                                pos[1],
                                pos[2]
                            ),
                            day_speed: None,
                            object_id: Some(id),
                            object_position: Some(pos),
                        },
                        None => CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: false,
                            message: format!(
                                "no {} found in loaded chunks",
                                match kind {
                                    ObjectKind::House => "houses",
                                    ObjectKind::Tree => "trees",
                                    ObjectKind::Fern => "ferns",
                                }
                            ),
                            day_speed: None,
                            object_id: None,
                            object_position: None,
                        },
                    }
                }
                CommandKind::LookAtObject {
                    ref object_id,
                    distance,
                } => {
                    let dist = distance.unwrap_or(15.0);
                    let result = parse_and_find_object(object_id, self.world.chunks());

                    match result {
                        Some(target) => {
                            let offset = glam::Vec3::new(1.0, 0.5, 1.0).normalize() * dist;
                            let cam_pos = target + offset;
                            self.camera.position = cam_pos;

                            let to_target = target - cam_pos;
                            self.camera.yaw = to_target.z.atan2(to_target.x);
                            self.camera.pitch = (to_target.y / to_target.length().max(0.001))
                                .asin()
                                .clamp(-1.54, 1.54);

                            CommandAppliedEvent {
                                id: command.id,
                                frame: self.frame_index,
                                ok: true,
                                message: format!(
                                    "looking at ({:.1}, {:.1}, {:.1}) from {:.1}m",
                                    target.x, target.y, target.z, dist
                                ),
                                day_speed: None,
                                object_id: Some(object_id.clone()),
                                object_position: Some([target.x, target.y, target.z]),
                            }
                        }
                        None => CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: false,
                            message: format!("object '{}' not found", object_id),
                            day_speed: None,
                            object_id: None,
                            object_position: None,
                        },
                    }
                }
                CommandKind::TakeScreenshot => {
                    if self.screenshot_pending.is_some() {
                        CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: false,
                            message: "screenshot already pending".to_string(),
                            day_speed: None,
                            object_id: None,
                            object_position: None,
                        }
                    } else {
                        self.screenshot_pending = Some(command.id);
                        continue;
                    }
                }
            };

            api.publish_command_applied(applied);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn publish_telemetry_if_due(&mut self, stats: &RuntimeStats) {
        let Some(api) = &self.debug_api else {
            return;
        };

        if self.last_telemetry_emit.elapsed() < Duration::from_millis(100) {
            return;
        }

        let telemetry = TelemetrySnapshot {
            frame: self.frame_index,
            frame_time_ms: self.frame_time_ms,
            fps: 1000.0 / self.frame_time_ms.max(0.01),
            hour: stats.hour,
            day_speed: self.world.day_speed(),
            camera: CameraSnapshot {
                x: self.camera.position.x,
                y: self.camera.position.y,
                z: self.camera.position.z,
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
            },
            chunks: ChunkSnapshot {
                loaded: stats.loaded_chunks,
                pending: stats.pending_chunks,
                center: [stats.center_chunk.x, stats.center_chunk.y],
            },
            timestamp_ms: now_timestamp_ms(),
        };

        api.publish_telemetry(telemetry);
        self.last_telemetry_emit = Instant::now();
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

    #[cfg(not(target_arch = "wasm32"))]
    fn handle_screenshot(
        &mut self,
        command_id: String,
        texture: &wgpu::Texture,
        mut encoder: wgpu::CommandEncoder,
    ) {
        let width = self.gpu.config.width;
        let height = self.gpu.config.height;
        let bytes_per_pixel = 4u32;
        let unpadded_row = width * bytes_per_pixel;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = unpadded_row.div_ceil(align) * align;
        let buffer_size = (padded_row * height) as u64;

        let staging = self.gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot-staging"),
            size: buffer_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &staging,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.gpu.queue.submit(Some(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        self.gpu.device.poll(wgpu::Maintain::Wait);

        let result = rx
            .recv()
            .map_err(|_| "channel closed".to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));

        let (ok, message) = match result {
            Ok(()) => {
                let data = slice.get_mapped_range();
                let is_bgra = matches!(
                    self.gpu.config.format,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                );
                match save_screenshot(&data, width, height, padded_row, unpadded_row, is_bgra) {
                    Ok(filename) => (true, format!("screenshot saved: {filename}")),
                    Err(e) => (false, format!("screenshot save failed: {e}")),
                }
            }
            Err(e) => (false, format!("screenshot readback failed: {e}")),
        };

        if let Some(api) = &self.debug_api {
            api.publish_command_applied(CommandAppliedEvent {
                id: command_id,
                frame: self.frame_index,
                ok,
                message,
                day_speed: None,
                object_id: None,
                object_position: None,
            });
        }
    }
}

pub fn run_event_loop(mut app: AppState, event_loop: EventLoop<()>) -> Result<()> {
    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent { window_id, event } if window_id == app.window.id() => {
                app.process_window_event(&event);

                match event {
                    WindowEvent::CloseRequested => {
                        #[cfg(not(target_arch = "wasm32"))]
                        app.save_game();
                        target.exit();
                    }
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape)) =>
                    {
                        app.release_cursor();
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    } if app.focused && !app.cursor_captured => {
                        app.capture_cursor();
                    }
                    WindowEvent::Resized(size) => app.resize(size),
                    WindowEvent::RedrawRequested => {
                        app.update();
                        match app.render() {
                            Ok(()) => {}
                            Err(SurfaceError::Lost) => app.resize(app.gpu.size),
                            Err(SurfaceError::OutOfMemory) => target.exit(),
                            Err(SurfaceError::Timeout | SurfaceError::Outdated) => {}
                        }
                    }
                    _ => {}
                }
            }
            Event::DeviceEvent { event, .. } => {
                app.process_device_event(&event);
            }
            Event::AboutToWait => {
                app.window.request_redraw();
            }
            _ => {}
        }
    })?;

    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub fn run_event_loop_web(window: &'static Window, event_loop: EventLoop<()>) {
    use std::cell::RefCell;
    use std::rc::Rc;
    use winit::platform::web::EventLoopExtWebSys;

    let app: Rc<RefCell<Option<AppState>>> = Rc::new(RefCell::new(None));
    let init_started = Rc::new(RefCell::new(false));

    let app_for_loop = Rc::clone(&app);
    let init_started_for_loop = Rc::clone(&init_started);

    event_loop.spawn(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);

        // On first Resumed event, start async GPU init
        if matches!(event, Event::Resumed) && !*init_started_for_loop.borrow() {
            *init_started_for_loop.borrow_mut() = true;
            let app_ref = Rc::clone(&app_for_loop);
            wasm_bindgen_futures::spawn_local(async move {
                // Don't grab cursor here — pointer lock requires a user gesture on web.
                // Cursor will be captured on first mouse click via the event loop.
                match AppState::new_web(window, false).await {
                    Ok(state) => {
                        *app_ref.borrow_mut() = Some(state);
                        log::info!("GPU initialized");
                    }
                    Err(e) => {
                        log::error!("failed to init: {e}");
                    }
                }
            });
            return;
        }

        let mut app_borrow = app_for_loop.borrow_mut();
        let Some(app) = app_borrow.as_mut() else {
            return;
        };

        match event {
            Event::WindowEvent { window_id, event } if window_id == app.window.id() => {
                app.process_window_event(&event);

                match event {
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape)) =>
                    {
                        app.release_cursor();
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    } if app.focused && !app.cursor_captured => {
                        app.capture_cursor();
                    }
                    WindowEvent::Resized(size) => app.resize(size),
                    WindowEvent::RedrawRequested => {
                        app.update();
                        match app.render() {
                            Ok(()) => {}
                            Err(SurfaceError::Lost) => app.resize(app.gpu.size),
                            Err(SurfaceError::OutOfMemory) => {
                                log::error!("out of GPU memory");
                            }
                            Err(SurfaceError::Timeout | SurfaceError::Outdated) => {}
                        }
                    }
                    _ => {}
                }
            }
            Event::DeviceEvent { event, .. } => {
                app.process_device_event(&event);
            }
            Event::AboutToWait => {
                app.window.request_redraw();
            }
            _ => {}
        }
    });
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

#[cfg(not(target_arch = "wasm32"))]
fn save_screenshot(
    data: &[u8],
    width: u32,
    height: u32,
    padded_row: u32,
    unpadded_row: u32,
    bgra: bool,
) -> Result<String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut pixels = Vec::with_capacity((unpadded_row * height) as usize);
    for row in 0..height {
        let offset = (row * padded_row) as usize;
        let row_bytes = &data[offset..offset + unpadded_row as usize];
        if bgra {
            for chunk in row_bytes.chunks_exact(4) {
                pixels.extend_from_slice(&[chunk[2], chunk[1], chunk[0], chunk[3]]);
            }
        } else {
            pixels.extend_from_slice(row_bytes);
        }
    }

    std::fs::create_dir_all("captures").context("failed to create captures dir")?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    let filename = format!(
        "world-gen-{:04}{:02}{:02}-{:02}{:02}{:02}.png",
        y, m, d, hours, minutes, seconds,
    );
    let path = std::path::Path::new("captures").join(&filename);
    let latest = std::path::Path::new("captures").join("latest.png");

    image::save_buffer(&path, &pixels, width, height, image::ColorType::Rgba8)
        .context("failed to encode PNG")?;
    let _ = std::fs::copy(&path, &latest);

    log::info!("screenshot saved: {}", path.display());
    Ok(filename)
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

#[cfg(not(target_arch = "wasm32"))]
fn parse_and_find_object(
    object_id: &str,
    chunks: &std::collections::HashMap<glam::IVec2, crate::world_core::chunk::ChunkData>,
) -> Option<Vec3> {
    // Format: "{type}-{chunk_x}_{chunk_z}-{index}"
    // Use first '-' and last '-' to handle negative chunk coordinates (e.g. "fern--2_3-0")
    let first_dash = object_id.find('-')?;
    let kind = &object_id[..first_dash];
    let rest = &object_id[first_dash + 1..];
    let last_dash = rest.rfind('-')?;
    let coord_str = &rest[..last_dash];
    let index_str = &rest[last_dash + 1..];

    let underscore = coord_str.find('_')?;
    let cx: i32 = coord_str[..underscore].parse().ok()?;
    let cz: i32 = coord_str[underscore + 1..].parse().ok()?;

    let index: usize = index_str.parse().ok()?;
    let chunk = chunks.get(&glam::IVec2::new(cx, cz))?;

    match kind {
        "house" => chunk.content.houses.get(index).map(|h| h.position),
        "tree" => chunk.content.trees.get(index).map(|t| t.position),
        "fern" => chunk.content.ferns.get(index).map(|f| f.position),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn now_timestamp_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
