use anyhow::Result;
use glam::Vec3;
use wgpu::SurfaceError;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, WindowEvent};
use winit::window::{CursorGrabMode, Window};

use crate::renderer_wgpu::blur_pass::BlurPass;
use crate::renderer_wgpu::camera::{CameraController, FlyCamera};
use crate::renderer_wgpu::egui_bridge::EguiBridge;
use crate::renderer_wgpu::egui_pass::EguiPass;
use crate::renderer_wgpu::gpu_context::GpuContext;
use crate::renderer_wgpu::thumbnail::ThumbnailRenderer;
use crate::renderer_wgpu::world::WorldRenderer;
use crate::ui::plant_editor_panel::PlantParams;
use crate::ui::{ConfigPanel, HerbariumUi, MenuAction, PlantEditorPanel, StartMenu, UiRegistry};
use crate::world_core::config::GameConfig;
use crate::world_core::herbarium::Herbarium;
use crate::world_core::save::{CameraSave, SaveData, WorldSave};
use crate::world_core::storage::{create_storage, Storage};
use crate::world_runtime::WorldRuntime;
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
pub(crate) mod plant_editor;
#[cfg(not(target_arch = "wasm32"))]
mod screenshot;

pub use event_loop::run_event_loop;
#[cfg(target_arch = "wasm32")]
pub use event_loop::run_event_loop_web;

enum Screen {
    StartMenu,
    Loading,
    Playing,
    Herbarium,
    PlantEditor,
}

#[derive(Clone, Copy)]
enum LoadingPhase {
    Init,
    BuildRegistry,
    CreateWorld,
    DispatchChunks,
    WaitForChunks,
    SyncTerrain,
    SyncWater,
    SyncInstances,
    SyncMinimap,
    Done,
}

struct LoadingState {
    phase: LoadingPhase,
    resume: bool,
    progress: f32,
}

pub struct AppState {
    window: &'static Window,
    gpu: GpuContext,
    world_renderer: WorldRenderer,
    camera: FlyCamera,
    camera_controller: CameraController,
    world: Option<WorldRuntime>,
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
    egui_bridge: EguiBridge,
    egui_pass: EguiPass,
    config_panel: ConfigPanel,
    plant_editor_panel: PlantEditorPanel,
    plant_editor: Option<plant_editor::PlantEditorState>,
    screen: Screen,
    start_menu: StartMenu,
    herbarium: Herbarium,
    herbarium_ui: HerbariumUi,
    editing_plant_index: Option<usize>,
    storage: Box<dyn Storage>,
    save: Option<SaveData>,
    config: GameConfig,
    pending_menu_action: Option<MenuAction>,
    ui_registry: UiRegistry,
    loading_state: Option<LoadingState>,
    loading_registry: Option<std::sync::Arc<crate::world_core::herbarium::PlantRegistry>>,
    thumbnail_renderer: Option<ThumbnailRenderer>,
    blur_pass: BlurPass,
    blur_capture_pending: bool,
}

impl AppState {
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new(
        window: &'static Window,
        debug_api_config: DebugApiConfig,
        _cursor_captured: bool,
    ) -> Result<Self> {
        let storage = create_storage();
        let config = GameConfig::load(&*storage);
        let save = SaveData::load(&*storage);

        let gpu = GpuContext::new(window).await?;

        let herbarium = Herbarium::load(&*storage);
        let registry = crate::world_core::herbarium::PlantRegistry::from_herbarium(&herbarium);

        let world_renderer = WorldRenderer::new(
            &gpu.device,
            &gpu.queue,
            &gpu.config,
            gpu.render_format,
            config.sea_level,
            config.world.load_radius,
            registry,
        );

        // Menu camera — fixed position looking at the sky
        let camera = FlyCamera::new(Vec3::new(96.0, 150.0, 16.0));
        let camera_controller = CameraController::new(180.0, 0.0022);

        // World is deferred until the player clicks Start/Resume
        let save_exists = save.is_some();

        let debug_api = start_debug_api(&debug_api_config)?;
        if let Some(api) = &debug_api {
            log::info!("debug api listening on {}", api.bind_addr());
        }

        let asset_watcher = AssetWatcher::start();

        let scale_factor = window.scale_factor() as f32;
        let egui_bridge = EguiBridge::new(scale_factor, gpu.config.width, gpu.config.height);
        let egui_pass = EguiPass::new(&gpu.device, gpu.render_format);
        let config_panel = ConfigPanel::new(&config);
        let blur_pass = BlurPass::new(
            &gpu.device,
            gpu.render_format,
            gpu.config.width,
            gpu.config.height,
        );

        Ok(Self {
            window,
            gpu,
            world_renderer,
            camera,
            camera_controller,
            world: None,
            debug_api,
            focused: true,
            cursor_captured: false,
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
            plant_editor_panel: PlantEditorPanel::default(),
            plant_editor: None,
            screen: Screen::StartMenu,
            start_menu: StartMenu::new(save_exists),
            herbarium,
            herbarium_ui: HerbariumUi,
            editing_plant_index: None,
            storage,
            save,
            config,
            pending_menu_action: None,
            ui_registry: UiRegistry::new(),
            loading_state: None,
            loading_registry: None,
            thumbnail_renderer: None,
            blur_pass,
            blur_capture_pending: false,
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new_web(window: &'static Window, _cursor_captured: bool) -> Result<Self> {
        let storage = create_storage();
        let config = GameConfig::load(&*storage);
        let save = SaveData::load(&*storage);

        let gpu = GpuContext::new(window).await?;

        let herbarium = Herbarium::load(&*storage);
        let registry = crate::world_core::herbarium::PlantRegistry::from_herbarium(&herbarium);

        let world_renderer = WorldRenderer::new(
            &gpu.device,
            &gpu.queue,
            &gpu.config,
            gpu.render_format,
            config.sea_level,
            config.world.load_radius,
            registry,
        );

        // Menu camera — fixed position looking at the sky
        let camera = FlyCamera::new(Vec3::new(96.0, 150.0, 16.0));
        let camera_controller = CameraController::new(180.0, 0.0022);

        let scale_factor = window.scale_factor() as f32;
        let egui_bridge = EguiBridge::new(scale_factor, gpu.config.width, gpu.config.height);
        let egui_pass = EguiPass::new(&gpu.device, gpu.render_format);
        let config_panel = ConfigPanel::new(&config);
        let save_exists = save.is_some();
        let blur_pass = BlurPass::new(
            &gpu.device,
            gpu.render_format,
            gpu.config.width,
            gpu.config.height,
        );

        Ok(Self {
            window,
            gpu,
            world_renderer,
            camera,
            camera_controller,
            world: None,
            focused: true,
            cursor_captured: false,
            last_frame: Instant::now(),
            frame_time_ms: 0.0,
            elapsed_seconds: 0.0,
            frame_index: 0,
            egui_bridge,
            egui_pass,
            config_panel,
            plant_editor_panel: PlantEditorPanel::default(),
            plant_editor: None,
            screen: Screen::StartMenu,
            start_menu: StartMenu::new(save_exists),
            herbarium,
            herbarium_ui: HerbariumUi,
            editing_plant_index: None,
            storage,
            save,
            config,
            pending_menu_action: None,
            ui_registry: UiRegistry::new(),
            loading_state: None,
            loading_registry: None,
            thumbnail_renderer: None,
            blur_pass,
            blur_capture_pending: false,
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

    fn is_on_menu(&self) -> bool {
        matches!(self.screen, Screen::StartMenu)
    }

    fn is_loading(&self) -> bool {
        matches!(self.screen, Screen::Loading)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn screen_name(&self) -> &'static str {
        match self.screen {
            Screen::StartMenu => "start_menu",
            Screen::Loading => "loading",
            Screen::Playing => "playing",
            Screen::Herbarium => "herbarium",
            Screen::PlantEditor => "plant_editor",
        }
    }

    fn is_on_editor(&self) -> bool {
        matches!(self.screen, Screen::PlantEditor)
    }

    fn is_on_herbarium(&self) -> bool {
        matches!(self.screen, Screen::Herbarium)
    }

    fn return_to_menu(&mut self) {
        // Capture the current frame for a blurred menu background (desktop only;
        // WASM surface textures lack COPY_SRC).
        #[cfg(not(target_arch = "wasm32"))]
        if self.world.is_some() {
            self.blur_capture_pending = true;
        }
        self.screen = Screen::StartMenu;
        self.start_menu.set_save_exists(true);
    }

    fn enter_herbarium(&mut self) {
        self.screen = Screen::Herbarium;
        self.generate_thumbnails();
    }

    fn generate_thumbnails(&mut self) {
        let renderer = self
            .thumbnail_renderer
            .get_or_insert_with(|| ThumbnailRenderer::new(&self.gpu.device));
        let seed = self.config.world.seed;
        renderer.generate_all(
            &self.gpu.device,
            &self.gpu.queue,
            &self.herbarium,
            seed,
            self.egui_pass.renderer_mut(),
        );
    }

    fn leave_herbarium(&mut self) {
        self.screen = Screen::StartMenu;
    }

    fn enter_plant_editor_for_entry(&mut self, index: usize) {
        self.screen = Screen::PlantEditor;
        self.editing_plant_index = Some(index);

        let species = &self.herbarium.plants[index].species;
        let mut editor =
            plant_editor::PlantEditorState::new(&self.gpu.device, self.config.world.seed, species);

        let initial_params = PlantParams::from_species(species);
        self.plant_editor_panel.set_params(initial_params.clone());
        editor.request_generation(&initial_params);

        let screen_w = self.gpu.config.width as f32 / self.egui_bridge.pixels_per_point();
        let (cam_pos, yaw, pitch) =
            editor.orbit_camera(screen_w, self.camera.fov_y_radians, self.gpu.aspect());
        self.camera = FlyCamera::new(cam_pos);
        self.camera.yaw = yaw;
        self.camera.pitch = pitch;

        self.plant_editor = Some(editor);
    }

    fn enter_plant_editor_new_plant(&mut self) {
        let name = format!("Plant {}", self.herbarium.plants.len() + 1);
        let entry = crate::world_core::herbarium::Herbarium::new_entry(name);
        self.herbarium.plants.push(entry);
        let index = self.herbarium.plants.len() - 1;
        self.enter_plant_editor_for_entry(index);
    }

    fn leave_plant_editor(&mut self) {
        // Save current editor state back to herbarium entry
        if let (Some(editor), Some(index)) = (&self.plant_editor, self.editing_plant_index) {
            let params = self.plant_editor_panel.current_params();
            let species = editor.current_species(params);
            if let Some(entry) = self.herbarium.plants.get_mut(index) {
                entry.species = species;
            }
            if let Err(e) = self.herbarium.save(&*self.storage) {
                log::warn!("failed to save herbarium: {e}");
            }
        }
        if let Some(index) = self.editing_plant_index {
            if let Some(thumb) = &mut self.thumbnail_renderer {
                let seed = self.config.world.seed;
                thumb.invalidate(
                    index,
                    &self.gpu.device,
                    &self.gpu.queue,
                    &self.herbarium,
                    seed,
                    self.egui_pass.renderer_mut(),
                );
            }
        }
        self.plant_editor = None;
        self.editing_plant_index = None;
        self.screen = Screen::Herbarium;
    }

    fn delete_current_plant(&mut self) {
        if let Some(index) = self.editing_plant_index {
            if index < self.herbarium.plants.len() {
                self.herbarium.plants.remove(index);
                if let Err(e) = self.herbarium.save(&*self.storage) {
                    log::warn!("failed to save herbarium after delete: {e}");
                }
            }
        }
        self.plant_editor = None;
        self.editing_plant_index = None;
        self.screen = Screen::Herbarium;
        self.generate_thumbnails();
    }

    fn begin_loading(&mut self, resume: bool) {
        // If resuming and the world is still alive in memory, just switch back
        if resume && self.world.is_some() {
            self.screen = Screen::Playing;
            self.capture_cursor();
            return;
        }

        self.blur_pass.clear_result();
        self.screen = Screen::Loading;
        self.loading_state = Some(LoadingState {
            phase: LoadingPhase::Init,
            resume,
            progress: 0.0,
        });
    }

    fn tick_loading(&mut self) {
        let Some(state) = &self.loading_state else {
            return;
        };
        let phase = state.phase;
        let resume = state.resume;

        match phase {
            LoadingPhase::Init => {
                let save_ref = if resume { self.save.as_ref() } else { None };
                let (cam_pos, cam_yaw, cam_pitch) = match save_ref {
                    Some(s) => (
                        Vec3::new(
                            s.camera.position[0],
                            s.camera.position[1],
                            s.camera.position[2],
                        ),
                        s.camera.yaw,
                        s.camera.pitch,
                    ),
                    None => (Vec3::new(158.0, 72.0, -51.0), 4.0, -0.23),
                };
                self.camera = FlyCamera::new(cam_pos);
                self.camera.yaw = cam_yaw;
                self.camera.pitch = cam_pitch;

                self.world = None;
                self.world_renderer
                    .clear_chunks(&self.gpu.device, &self.gpu.queue);

                if let Some(s) = &mut self.loading_state {
                    s.phase = LoadingPhase::BuildRegistry;
                    s.progress = 0.05;
                }
            }
            LoadingPhase::BuildRegistry => {
                let registry =
                    crate::world_core::herbarium::PlantRegistry::from_herbarium(&self.herbarium);
                let arc_registry = std::sync::Arc::new(registry);
                self.world_renderer.update_registry(
                    &self.gpu.device,
                    crate::world_core::herbarium::PlantRegistry::clone(&arc_registry),
                );
                self.loading_registry = Some(arc_registry);

                if let Some(s) = &mut self.loading_state {
                    s.phase = LoadingPhase::CreateWorld;
                    s.progress = 0.15;
                }
            }
            LoadingPhase::CreateWorld => {
                let save_ref = if resume { self.save.as_ref() } else { None };

                #[cfg(not(target_arch = "wasm32"))]
                let threads = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4);
                #[cfg(target_arch = "wasm32")]
                let threads = 1;

                let arc_registry = self.loading_registry.take().unwrap_or_else(|| {
                    std::sync::Arc::new(
                        crate::world_core::herbarium::PlantRegistry::from_herbarium(
                            &self.herbarium,
                        ),
                    )
                });
                match WorldRuntime::new(&self.config, save_ref, threads, arc_registry) {
                    Ok(world) => {
                        self.world = Some(world);
                    }
                    Err(err) => {
                        log::error!("failed to create world runtime: {err}");
                        self.screen = Screen::StartMenu;
                        self.loading_state = None;
                        return;
                    }
                }

                if let Some(s) = &mut self.loading_state {
                    s.phase = LoadingPhase::DispatchChunks;
                    s.progress = 0.30;
                }
            }
            LoadingPhase::DispatchChunks => {
                if let Some(world) = &mut self.world {
                    world.update(0.0, self.camera.position);
                }

                if let Some(s) = &mut self.loading_state {
                    s.phase = LoadingPhase::WaitForChunks;
                    s.progress = 0.40;
                }
            }
            LoadingPhase::WaitForChunks => {
                // Poll for completed chunks each frame until all are loaded
                if let Some(world) = &mut self.world {
                    world.update(0.0, self.camera.position);
                    let stats = world.stats();
                    if stats.pending_chunks == 0 {
                        if let Some(s) = &mut self.loading_state {
                            s.phase = LoadingPhase::SyncTerrain;
                            s.progress = 0.55;
                        }
                    } else {
                        // Show progress based on how many chunks have loaded
                        let total = stats.loaded_chunks + stats.pending_chunks;
                        let frac = stats.loaded_chunks as f32 / total.max(1) as f32;
                        if let Some(s) = &mut self.loading_state {
                            s.progress = 0.40 + frac * 0.15;
                        }
                    }
                }
            }
            LoadingPhase::SyncTerrain => {
                if let Some(world) = &self.world {
                    self.world_renderer.sync_terrain(
                        &self.gpu.device,
                        &self.gpu.queue,
                        world.chunks(),
                    );
                }

                if let Some(s) = &mut self.loading_state {
                    s.phase = LoadingPhase::SyncWater;
                    s.progress = 0.70;
                }
            }
            LoadingPhase::SyncWater => {
                if let Some(world) = &self.world {
                    self.world_renderer
                        .sync_water(&self.gpu.device, world.chunks());
                }

                if let Some(s) = &mut self.loading_state {
                    s.phase = LoadingPhase::SyncInstances;
                    s.progress = 0.80;
                }
            }
            LoadingPhase::SyncInstances => {
                if let Some(world) = &self.world {
                    self.world_renderer
                        .sync_instances(&self.gpu.device, world.chunks());
                }

                if let Some(s) = &mut self.loading_state {
                    s.phase = LoadingPhase::SyncMinimap;
                    s.progress = 0.92;
                }
            }
            LoadingPhase::SyncMinimap => {
                if let Some(world) = &self.world {
                    self.world_renderer
                        .sync_minimap(&self.gpu.queue, world.chunks());
                }

                if let Some(s) = &mut self.loading_state {
                    s.phase = LoadingPhase::Done;
                    s.progress = 1.0;
                }
            }
            LoadingPhase::Done => {
                self.screen = Screen::Playing;
                self.capture_cursor();
                self.loading_state = None;
            }
        }
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.gpu.resize(new_size);
        self.world_renderer
            .resize(&self.gpu.device, &self.gpu.config);
        self.egui_bridge
            .resize(self.gpu.config.width, self.gpu.config.height);
        self.blur_pass.resize(
            &self.gpu.device,
            &self.gpu.queue,
            self.gpu.config.width,
            self.gpu.config.height,
        );
    }

    fn update(&mut self) {
        self.frame_index = self.frame_index.saturating_add(1);

        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.frame_time_ms = self.frame_time_ms * 0.94 + (dt * 1000.0) * 0.06;
        self.elapsed_seconds += dt;

        if self.is_loading() {
            self.update_menu(dt);
            self.tick_loading();
            return;
        }

        if self.is_on_menu() || self.is_on_herbarium() {
            self.update_menu(dt);
            return;
        }

        if self.is_on_editor() {
            self.update_editor(dt);
            return;
        }

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

        self.camera_controller.update_camera(
            dt,
            &mut self.camera,
            self.focused && self.cursor_captured,
        );

        let world = self.world.as_mut().unwrap();

        clamp_camera_to_terrain(&mut self.camera, world.chunks());

        world.update(dt, self.camera.position);
        self.world_renderer
            .sync_chunks(&self.gpu.device, &self.gpu.queue, world.chunks());

        let aspect = self.gpu.aspect();
        let view_proj = self.camera.view_projection(aspect);
        let lighting = world.lighting();
        let stats = world.stats();
        let palette = crate::renderer_wgpu::sky::sky_palette(stats.hour);
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
            &palette,
        );
        self.world_renderer.update_hud(
            &self.gpu.queue,
            &self.gpu.device,
            self.camera.position,
            self.camera.yaw,
            stats.hour,
            self.gpu.config.width as f32,
            self.gpu.config.height as f32,
        );
        self.world_renderer.update_minimap(
            &self.gpu.queue,
            &self.gpu.device,
            dt,
            self.camera.position,
            self.camera.yaw,
            self.camera.fov_y_radians,
            self.gpu.config.width as f32,
            self.gpu.config.height as f32,
        );

        // Apply config panel changes (debounced — only on pointer release)
        if let Some(new_config) = self.config_panel.take_dirty_config(self.egui_bridge.ctx()) {
            let world = self.world.as_mut().unwrap();
            world.reload_config(&new_config);
            self.world_renderer
                .set_sea_level(&self.gpu.queue, new_config.sea_level);
            self.world_renderer
                .set_load_radius(new_config.world.load_radius);
            let _ = world.set_day_speed(new_config.world.day_speed);
        }

        #[cfg(not(target_arch = "wasm32"))]
        self.publish_telemetry_if_due(&stats);

        let day_speed = self.world.as_ref().unwrap().day_speed();
        self.window.set_title(&format!(
            "world-gen | {:.1}ms ({:.0}fps) | chunks: {}/{} | center: {},{} | hour: {:.1} | day_speed: {:.2}",
            self.frame_time_ms,
            1000.0 / self.frame_time_ms.max(0.01),
            stats.loaded_chunks,
            stats.loaded_chunks + stats.pending_chunks,
            stats.center_chunk.x,
            stats.center_chunk.y,
            stats.hour,
            day_speed,
        ));
    }

    fn update_editor(&mut self, dt: f32) {
        // Update orbit camera
        if let Some(editor) = &mut self.plant_editor {
            editor.update_orbit(dt);
            let screen_w = self.gpu.config.width as f32 / self.egui_bridge.pixels_per_point();
            let (cam_pos, yaw, pitch) =
                editor.orbit_camera(screen_w, self.camera.fov_y_radians, self.gpu.aspect());
            self.camera.position = cam_pos;
            self.camera.yaw = yaw;
            self.camera.pitch = pitch;
        }

        // Fixed noon lighting
        let hour = 12.0;
        let angle = (hour / 24.0) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let altitude = angle.sin();
        let azimuth = angle.cos();
        let light_dir = Vec3::new(azimuth * 0.45, altitude, 0.75).normalize();
        let day = (light_dir.y * 0.5 + 0.5).clamp(0.0, 1.0);
        let ambient = 0.1 + day * 0.35;

        let aspect = self.gpu.aspect();
        let view_proj = self.camera.view_projection(aspect);
        let palette = crate::renderer_wgpu::sky::sky_palette(hour);

        self.world_renderer.update_frame(
            &self.gpu.queue,
            view_proj,
            self.camera.position,
            self.elapsed_seconds,
            hour,
        );
        self.world_renderer
            .update_material(&self.gpu.queue, light_dir, ambient, &palette);

        // Process debug commands in editor mode
        #[cfg(not(target_arch = "wasm32"))]
        self.apply_editor_debug_commands();

        // Check for dirty params (slider/combo changes)
        if let Some(params) = self
            .plant_editor_panel
            .take_dirty_params(self.egui_bridge.ctx())
        {
            if let Some(editor) = &mut self.plant_editor {
                editor.request_generation(&params);
            }
        }

        // Poll for completed generation
        if let Some(editor) = &mut self.plant_editor {
            if let Some(plant_mesh) = editor.generator.poll() {
                editor.load_plant_mesh(&self.gpu.device, &plant_mesh);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn apply_editor_debug_commands(&mut self) {
        use crate::debug_api::{CommandAppliedEvent, CommandKind, MoveKey};

        let commands: Vec<_> = self
            .debug_api
            .as_mut()
            .map(|api| api.drain_commands())
            .unwrap_or_default();

        for command in commands {
            let applied = match command.command {
                CommandKind::TakeScreenshot => {
                    self.screenshot_pending = Some(command.id);
                    continue;
                }
                CommandKind::SetMoveKey { key, pressed } => {
                    if let Some(editor) = &mut self.plant_editor {
                        match key {
                            MoveKey::A => {
                                editor.orbit_left = pressed;
                                if pressed {
                                    editor.stop_auto_orbit();
                                }
                            }
                            MoveKey::D => {
                                editor.orbit_right = pressed;
                                if pressed {
                                    editor.stop_auto_orbit();
                                }
                            }
                            _ => {}
                        }
                    }
                    CommandAppliedEvent::ok(
                        command.id,
                        self.frame_index,
                        format!(
                            "orbit key {} {}",
                            key.as_str(),
                            if pressed { "pressed" } else { "released" }
                        ),
                    )
                }
                CommandKind::UiSnapshot => {
                    let snapshot = self.ui_registry.take_snapshot(self.screen_name());
                    let data = serde_json::to_value(&snapshot).unwrap_or(serde_json::Value::Null);
                    let mut evt = CommandAppliedEvent::ok(
                        command.id,
                        self.frame_index,
                        format!(
                            "ui snapshot: {} elements on {}",
                            snapshot.elements.len(),
                            snapshot.screen
                        ),
                    );
                    evt.data = Some(data);
                    evt
                }
                CommandKind::UiClick { ref element_id } => {
                    if !self.ui_registry.has_element(element_id) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("ui click failed: element '{}' not found", element_id),
                        )
                    } else {
                        self.ui_registry.push_action(crate::ui::UiAction::Click {
                            element_id: element_id.clone(),
                        });
                        CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!("ui click queued: {}", element_id),
                        )
                    }
                }
                CommandKind::UiSetValue {
                    ref element_id,
                    ref value,
                } => {
                    if !self.ui_registry.has_element(element_id) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("ui set_value failed: element '{}' not found", element_id),
                        )
                    } else {
                        self.ui_registry.push_action(crate::ui::UiAction::SetValue {
                            element_id: element_id.clone(),
                            value: value.clone(),
                        });
                        CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!("ui set_value queued: {} = {}", element_id, value),
                        )
                    }
                }
                _ => CommandAppliedEvent::err(
                    command.id,
                    self.frame_index,
                    "command not available in plant editor".to_string(),
                ),
            };

            if let Some(api) = &self.debug_api {
                api.publish_command_applied(applied);
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn apply_menu_debug_commands(&mut self) {
        use crate::debug_api::{CommandAppliedEvent, CommandKind};

        let commands: Vec<_> = self
            .debug_api
            .as_mut()
            .map(|api| api.drain_commands())
            .unwrap_or_default();

        for command in commands {
            let applied = match command.command {
                CommandKind::TakeScreenshot => {
                    self.screenshot_pending = Some(command.id);
                    continue;
                }
                CommandKind::UiSnapshot => {
                    let snapshot = self.ui_registry.take_snapshot(self.screen_name());
                    let data = serde_json::to_value(&snapshot).unwrap_or(serde_json::Value::Null);
                    let mut evt = CommandAppliedEvent::ok(
                        command.id,
                        self.frame_index,
                        format!(
                            "ui snapshot: {} elements on {}",
                            snapshot.elements.len(),
                            snapshot.screen
                        ),
                    );
                    evt.data = Some(data);
                    evt
                }
                CommandKind::UiClick { ref element_id } => {
                    if !self.ui_registry.has_element(element_id) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("ui click failed: element '{}' not found", element_id),
                        )
                    } else {
                        self.ui_registry.push_action(crate::ui::UiAction::Click {
                            element_id: element_id.clone(),
                        });
                        CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!("ui click queued: {}", element_id),
                        )
                    }
                }
                CommandKind::UiSetValue {
                    ref element_id,
                    ref value,
                } => {
                    if !self.ui_registry.has_element(element_id) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("ui set_value failed: element '{}' not found", element_id),
                        )
                    } else {
                        self.ui_registry.push_action(crate::ui::UiAction::SetValue {
                            element_id: element_id.clone(),
                            value: value.clone(),
                        });
                        CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!("ui set_value queued: {} = {}", element_id, value),
                        )
                    }
                }
                _ => CommandAppliedEvent::err(
                    command.id,
                    self.frame_index,
                    "command not available on menu".to_string(),
                ),
            };

            if let Some(api) = &self.debug_api {
                api.publish_command_applied(applied);
            }
        }
    }

    fn update_menu(&mut self, _dt: f32) {
        // Process debug commands on menu screen
        #[cfg(not(target_arch = "wasm32"))]
        self.apply_menu_debug_commands();

        // Advance a virtual hour for the animated sky background
        let menu_day_speed = 0.5;
        let menu_hour = (self.elapsed_seconds * menu_day_speed) % 24.0;

        // Reuse WorldClock's sun direction formula
        let angle = (menu_hour / 24.0) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
        let altitude = angle.sin();
        let azimuth = angle.cos();
        let light_dir = Vec3::new(azimuth * 0.45, altitude, 0.75).normalize();
        let day = (light_dir.y * 0.5 + 0.5).clamp(0.0, 1.0);
        let ambient = 0.1 + day * 0.35;

        let aspect = self.gpu.aspect();
        let view_proj = self.camera.view_projection(aspect);
        let palette = crate::renderer_wgpu::sky::sky_palette(menu_hour);

        self.world_renderer.update_frame(
            &self.gpu.queue,
            view_proj,
            self.camera.position,
            self.elapsed_seconds,
            menu_hour,
        );
        self.world_renderer
            .update_material(&self.gpu.queue, light_dir, ambient, &palette);
    }

    fn render(&mut self) -> Result<(), SurfaceError> {
        let output = self.gpu.surface.get_current_texture()?;
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.gpu.render_format),
            ..Default::default()
        });

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("world-gen-render-encoder"),
            });

        let is_menu = self.is_on_menu();
        let is_loading = self.is_loading();
        let is_herbarium = self.is_on_herbarium();
        let is_editor = self.is_on_editor();

        // When we have a blurred background ready, blit it directly (no depth needed).
        // Otherwise, run the normal 3D render pass.
        let use_blur_blit =
            (is_menu || is_loading) && self.blur_pass.has_result() && !self.blur_capture_pending;

        if use_blur_blit {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("blur-blit-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            self.blur_pass.blit(&mut pass);
        } else {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("terrain-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(self.world_renderer.clear_color()),
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

            if is_editor {
                if let Some(editor) = &self.plant_editor {
                    let mut meshes = vec![(&editor.ground_mesh, &editor.ground_instance)];
                    if let (Some(m), Some(i)) = (&editor.tree_mesh, &editor.tree_instance) {
                        meshes.push((m, i));
                    }
                    self.world_renderer.render_editor_scene(&mut pass, &meshes);
                }
            } else if self.blur_capture_pending {
                // Render scene without HUD so we can capture it for the blur
                self.world_renderer.render_scene(&mut pass);
            } else if is_menu || is_loading || is_herbarium {
                self.world_renderer.render_sky_only(&mut pass);
            } else {
                self.world_renderer.render(&mut pass);
            }
        }

        // Capture the rendered frame and apply Gaussian blur
        if self.blur_capture_pending {
            // Only perform the blur capture when the surface format matches the blur pass format.
            // This avoids potential wgpu validation errors from copying between mismatched formats.
            if self.gpu.render_format == self.gpu.config.format {
                self.blur_pass
                    .capture_and_blur(&mut encoder, &output.texture, 6);
                // Blit the blurred result back onto the surface before egui draws
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("blur-blit-pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
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
                    self.blur_pass.blit(&mut pass);
                }
            }
            self.blur_capture_pending = false;
        }

        // egui overlay pass (renders on top of 3D scene)
        {
            self.ui_registry.clear();
            let show_egui = is_menu
                || is_loading
                || is_herbarium
                || self.config_panel.is_visible()
                || is_editor;
            if show_egui {
                let raw_input = self.egui_bridge.take_raw_input();
                let mut menu_action = None;
                let loading_progress = self
                    .loading_state
                    .as_ref()
                    .map(|s| s.progress)
                    .unwrap_or(0.0);
                let full_output = self
                    .egui_bridge
                    .ctx()
                    .run(raw_input, |ctx| match self.screen {
                        Screen::StartMenu => {
                            menu_action = self.start_menu.ui(ctx, &mut self.ui_registry);
                        }
                        Screen::Loading => {
                            render_loading_ui(ctx, loading_progress);
                        }
                        Screen::Herbarium => {
                            use crate::ui::herbarium_ui::HerbariumAction;
                            if let Some(ha) = self.herbarium_ui.ui(
                                ctx,
                                &self.herbarium,
                                &mut self.ui_registry,
                                self.thumbnail_renderer.as_ref(),
                            ) {
                                menu_action = Some(match ha {
                                    HerbariumAction::OpenPlant(i) => MenuAction::OpenPlantEditor(i),
                                    HerbariumAction::NewPlant => MenuAction::NewPlant,
                                    HerbariumAction::Back => MenuAction::LeaveHerbarium,
                                });
                            }
                        }
                        Screen::Playing => {
                            self.config_panel.ui(ctx, &mut self.ui_registry);
                        }
                        Screen::PlantEditor => {
                            if let Some(ea) = self.plant_editor_panel.ui(ctx, &mut self.ui_registry)
                            {
                                use crate::ui::plant_editor_panel::EditorAction;
                                menu_action = Some(match ea {
                                    EditorAction::Back => MenuAction::LeaveEditor,
                                    EditorAction::Delete => MenuAction::DeletePlant,
                                    #[cfg(not(target_arch = "wasm32"))]
                                    EditorAction::Screenshot => MenuAction::EditorScreenshot,
                                });
                            }
                        }
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

                self.pending_menu_action = menu_action;
            }
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

    fn save_game(&self) {
        let Some(world) = &self.world else { return };
        let save = Self::build_save_data(&self.camera, world);
        if let Err(e) = save.save(&*self.storage) {
            log::warn!("failed to save game state: {e}");
        }
    }

    /// Save to storage and update the in-memory save (for mid-session resume).
    fn save_and_update(&mut self) {
        let Some(world) = &self.world else { return };
        let save = Self::build_save_data(&self.camera, world);
        match save.save(&*self.storage) {
            Ok(()) => {
                self.save = Some(save);
            }
            Err(e) => {
                log::warn!("failed to save game state: {e}");
            }
        }
    }

    fn build_save_data(camera: &FlyCamera, world: &WorldRuntime) -> SaveData {
        SaveData {
            camera: CameraSave {
                position: [camera.position.x, camera.position.y, camera.position.z],
                yaw: camera.yaw,
                pitch: camera.pitch,
            },
            world: WorldSave {
                seed: world.seed(),
                hour: world.hour(),
                day_speed: world.day_speed(),
            },
        }
    }
}

fn render_loading_ui(ctx: &egui::Context, progress: f32) {
    let message = match progress {
        p if p < 0.15 => "Preparing the soil...",
        p if p < 0.35 => "Planting seeds...",
        p if p < 0.65 => "Growing forests...",
        p if p < 0.85 => "Carving rivers...",
        p if p < 0.95 => "Welcoming wildlife...",
        _ => "World ready!",
    };

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(egui::Color32::from_black_alpha(140)))
        .show(ctx, |ui| {
            let available = ui.available_size();
            ui.add_space(available.y * 0.45);

            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(message)
                        .size(22.0)
                        .color(egui::Color32::WHITE),
                );
                ui.add_space(12.0);
                ui.add(
                    egui::ProgressBar::new(progress)
                        .desired_width(300.0)
                        .animate(true),
                );
            });
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
