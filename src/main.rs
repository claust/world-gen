mod renderer;
mod world_gen;

use anyhow::{Context, Result};
use glam::{IVec2, Vec3};
use renderer::camera::{CameraController, FlyCamera};
use renderer::terrain::TerrainRenderer;
use std::collections::HashMap;
use std::time::Instant;
use wgpu::SurfaceError;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, ElementState, Event, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowBuilder};

use crate::world_gen::chunk::{ChunkData, ChunkGenerator};

struct AppState {
    window: &'static Window,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    size: PhysicalSize<u32>,
    terrain_renderer: TerrainRenderer,
    camera: FlyCamera,
    camera_controller: CameraController,
    chunks: HashMap<IVec2, ChunkData>,
    focused: bool,
    last_frame: Instant,
    frame_time_ms: f32,
}

impl AppState {
    async fn new(window: &'static Window) -> Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::default();
        let surface = instance
            .create_surface(window)
            .context("failed to create wgpu surface")?;

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .context("no suitable GPU adapter found")?;

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("world-gen-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::default(),
                },
                None,
            )
            .await
            .context("failed to request GPU device")?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(capabilities.formats[0]);

        let present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            capabilities.present_modes[0]
        };

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: capabilities.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let mut terrain_renderer = TerrainRenderer::new(&device, &surface_config);

        let camera = FlyCamera::new(Vec3::new(0.0, 160.0, 0.0));
        let camera_controller = CameraController::new(180.0, 0.0022);

        let generator = ChunkGenerator::new(42);
        let chunk = generator.generate_chunk(IVec2::ZERO);
        let mut chunks = HashMap::with_capacity(1);
        chunks.insert(IVec2::ZERO, chunk);

        terrain_renderer.sync_chunks(&device, &chunks);

        Ok(Self {
            window,
            surface,
            device,
            queue,
            surface_config,
            size,
            terrain_renderer,
            camera,
            camera_controller,
            chunks,
            focused: true,
            last_frame: Instant::now(),
            frame_time_ms: 0.0,
        })
    }

    fn process_window_event(&mut self, event: &WindowEvent) {
        let _ = self.camera_controller.process_window_event(event);

        if let WindowEvent::Focused(focused) = event {
            self.focused = *focused;
        }
    }

    fn process_device_event(&mut self, event: &DeviceEvent) {
        self.camera_controller.process_device_event(event);
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 {
            return;
        }

        self.size = new_size;
        self.surface_config.width = new_size.width;
        self.surface_config.height = new_size.height;
        self.surface.configure(&self.device, &self.surface_config);
        self.terrain_renderer
            .resize(&self.device, &self.surface_config);
    }

    fn update(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.frame_time_ms = self.frame_time_ms * 0.94 + (dt * 1000.0) * 0.06;

        self.camera_controller
            .update_camera(dt, &mut self.camera, self.focused);

        let aspect = self.surface_config.width as f32 / self.surface_config.height.max(1) as f32;
        let view_proj = self.camera.view_projection(aspect);
        self.terrain_renderer
            .update_uniforms(&self.queue, view_proj, Vec3::new(0.3, 0.9, 0.2), 0.22);

        self.window.set_title(&format!(
            "world-gen MVP | {:.1}ms ({:.0}fps) | chunks: {}",
            self.frame_time_ms,
            1000.0 / self.frame_time_ms.max(0.01),
            self.chunks.len()
        ));
    }

    fn render(&mut self) -> Result<(), SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.device
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
                        load: wgpu::LoadOp::Clear(crate::renderer::sky::clear_color()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.terrain_renderer.depth_view(),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            self.terrain_renderer.render(&mut pass);
        }

        self.queue.submit(Some(encoder.finish()));
        output.present();
        Ok(())
    }
}

fn try_grab_cursor(window: &Window) {
    let _ = window.set_cursor_grab(CursorGrabMode::Locked);
    let _ = window.set_cursor_grab(CursorGrabMode::Confined);
    window.set_cursor_visible(false);
}

fn main() -> Result<()> {
    env_logger::init();

    let event_loop = EventLoop::new()?;
    let window = Box::leak(Box::new(
        WindowBuilder::new()
            .with_title("world-gen")
            .with_inner_size(PhysicalSize::new(1600, 900))
            .build(&event_loop)
            .context("failed to create window")?,
    ));

    try_grab_cursor(window);

    let mut app = pollster::block_on(AppState::new(window))?;

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
                        target.exit();
                    }
                    WindowEvent::Resized(size) => app.resize(size),
                    WindowEvent::RedrawRequested => {
                        app.update();
                        match app.render() {
                            Ok(()) => {}
                            Err(SurfaceError::Lost) => app.resize(app.size),
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
