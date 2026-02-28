mod debug_api;
mod renderer_wgpu;
mod world_core;
mod world_runtime;

use anyhow::{Context, Result};
use glam::Vec3;
use renderer_wgpu::camera::{CameraController, FlyCamera, MoveDirection};
use renderer_wgpu::gpu_context::GpuContext;
use renderer_wgpu::world::WorldRenderer;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use wgpu::SurfaceError;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, Event, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowBuilder};

use crate::debug_api::{
    start_debug_api, CameraSnapshot, ChunkSnapshot, CommandAppliedEvent, CommandKind,
    DebugApiConfig, DebugApiHandle, MoveKey, TelemetrySnapshot,
};
use crate::world_runtime::{RuntimeStats, WorldRuntime};

struct AppState {
    window: &'static Window,
    gpu: GpuContext,
    world_renderer: WorldRenderer,
    camera: FlyCamera,
    camera_controller: CameraController,
    world: WorldRuntime,
    debug_api: Option<DebugApiHandle>,
    focused: bool,
    cursor_captured: bool,
    last_frame: Instant,
    last_telemetry_emit: Instant,
    frame_time_ms: f32,
    frame_index: u64,
}

impl AppState {
    async fn new(
        window: &'static Window,
        debug_api_config: DebugApiConfig,
        cursor_captured: bool,
    ) -> Result<Self> {
        let gpu = GpuContext::new(window).await?;

        let mut world_renderer = WorldRenderer::new(&gpu.device, &gpu.config);

        let mut camera = FlyCamera::new(Vec3::new(96.0, 150.0, 16.0));
        camera.yaw = 1.02;
        camera.pitch = -0.38;
        let camera_controller = CameraController::new(180.0, 0.0022);

        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let mut world = WorldRuntime::new(42, 1, threads, 9.5, 0.04)?;
        world.update(0.0, camera.position);

        world_renderer.sync_chunks(&gpu.device, world.chunks());

        let debug_api = start_debug_api(&debug_api_config)?;
        if let Some(api) = &debug_api {
            log::info!("debug api listening on {}", api.bind_addr());
        }

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
                    },
                    Err(message) => CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: false,
                        message,
                        day_speed: Some(self.world.day_speed()),
                    },
                },
                CommandKind::SetMoveKey { key, pressed } => {
                    let direction = match key {
                        MoveKey::W => MoveDirection::Forward,
                        MoveKey::A => MoveDirection::Left,
                        MoveKey::S => MoveDirection::Backward,
                        MoveKey::D => MoveDirection::Right,
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
                    }
                }
            };

            api.publish_command_applied(applied);
        }
    }

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
        self.apply_debug_commands();

        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.frame_time_ms = self.frame_time_ms * 0.94 + (dt * 1000.0) * 0.06;

        self.camera_controller.update_camera(
            dt,
            &mut self.camera,
            self.focused && self.cursor_captured,
        );

        self.world.update(dt, self.camera.position);
        self.world_renderer
            .sync_chunks(&self.gpu.device, self.world.chunks());

        let aspect = self.gpu.aspect();
        let view_proj = self.camera.view_projection(aspect);
        let lighting = self.world.lighting();
        self.world_renderer.update_uniforms(
            &self.gpu.queue,
            view_proj,
            lighting.sun_direction,
            lighting.ambient,
        );

        let stats = self.world.stats();
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

        self.gpu.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

fn try_grab_window_cursor(window: &Window) -> bool {
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

fn now_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn main() -> Result<()> {
    env_logger::init();
    let debug_api = DebugApiConfig::from_env_args()?;
    log::info!(
        "debug api enabled: {}, bind: {}",
        debug_api.enabled,
        debug_api.bind_addr
    );

    let event_loop = EventLoop::new()?;
    let window = Box::leak(Box::new(
        WindowBuilder::new()
            .with_title("world-gen")
            .with_inner_size(PhysicalSize::new(1600, 900))
            .build(&event_loop)
            .context("failed to create window")?,
    ));

    let cursor_captured = try_grab_window_cursor(window);
    let mut app = pollster::block_on(AppState::new(window, debug_api, cursor_captured))?;

    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent { window_id, event } if window_id == app.window.id() => {
                app.process_window_event(&event);

                match event {
                    WindowEvent::CloseRequested => target.exit(),
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
