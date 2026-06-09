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
use crate::ui::{
    ConfigPanel, HerbariumUi, MenuAction, PlantEditorPanel, SettingsPanel, StartMenu, UiRegistry,
};
use crate::world_core::config::GameConfig;
use crate::world_core::herbarium::Herbarium;
use crate::world_core::save::{CameraSave, SaveData, WorldSave, FAVORITE_SLOTS};
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
mod benchmark;
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

/// Background world-generation worker. The heavy `WorldRuntime::generate` runs on
/// this thread so the event loop keeps drawing the loading animation at 60fps;
/// the result arrives over `result`. Dropping this struct drops the receiver,
/// which detaches the thread — leaving the loading screen never blocks on a join.
#[cfg(not(target_arch = "wasm32"))]
struct WorldGenJob {
    result: std::sync::mpsc::Receiver<anyhow::Result<WorldRuntime>>,
    _handle: std::thread::JoinHandle<()>,
}

/// State of the base-world snapshot download kicked off at launch.
#[cfg(target_arch = "wasm32")]
enum BasePrefetch {
    /// The fetch is still running.
    InFlight,
    /// The fetch finished; `None` means it failed (a New Game falls back to local
    /// generation). The bytes are kept (not taken) so a loading retry reuses them.
    Ready(Option<Vec<u8>>),
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
    /// Value of `elapsed_seconds` at the last periodic autosave, so the playing
    /// loop persists progress on a fixed cadence between explicit saves (ESC /
    /// window close). Native only — the desktop build is where an unexpected exit
    /// would otherwise lose progress.
    #[cfg(not(target_arch = "wasm32"))]
    last_autosave_seconds: f32,
    frame_index: u64,
    #[cfg(not(target_arch = "wasm32"))]
    debug_api: Option<DebugApiHandle>,
    #[cfg(not(target_arch = "wasm32"))]
    last_telemetry_emit: Instant,
    #[cfg(not(target_arch = "wasm32"))]
    screenshot_pending: Option<String>,
    /// When set, the next pending screenshot is copied to the system clipboard
    /// (in addition to being saved to `captures/`). Set by the `P` shortcut.
    #[cfg(not(target_arch = "wasm32"))]
    screenshot_to_clipboard: bool,
    /// Brief on-screen thumbnail confirming a clipboard screenshot was taken;
    /// rests top-right, then slides out. See `draw_screenshot_toast`.
    #[cfg(not(target_arch = "wasm32"))]
    screenshot_toast: Option<ScreenshotToast>,
    /// Directory screenshots are written to: `captures` by default, or
    /// `instances/<name>/captures` when running as a named instance, so
    /// concurrent instances don't overwrite each other's `latest.png`.
    #[cfg(not(target_arch = "wasm32"))]
    captures_dir: std::path::PathBuf,
    #[cfg(not(target_arch = "wasm32"))]
    asset_watcher: Option<AssetWatcher>,
    /// Ambient sound (procedural surf + birdsong + underwater drone). `None` if
    /// no audio device is available; native only.
    #[cfg(not(target_arch = "wasm32"))]
    audio: Option<crate::audio::AudioSystem>,
    egui_bridge: EguiBridge,
    egui_pass: EguiPass,
    config_panel: ConfigPanel,
    settings_panel: SettingsPanel,
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
    /// Live progress of the async world build, shared with the worker thread and
    /// read each frame by the loading visualization.
    world_gen_progress: Option<std::sync::Arc<crate::world_runtime::GenerationProgress>>,
    /// Wall-clock start of the current world build, for the elapsed-time readout.
    loading_started: Option<Instant>,
    /// 256×256 biome map painted in as chunks finish; re-uploaded only when the
    /// chunk-done count changes.
    loading_map_tex: Option<egui::TextureHandle>,
    /// Reused RGBA scratch for the biome map, so the per-frame texture refresh
    /// doesn't allocate 256 KB every frame across the multi-second build.
    loading_map_buf: Vec<u8>,
    /// `done` count at the last map upload, to skip rebuilds when nothing changed.
    loading_map_done: usize,
    /// Full-world biome map retained after generation for the `M`-key map overlay.
    /// Adopted from `loading_map_tex` when the build finishes, so it covers every
    /// canonical chunk — not just the streamed-in ones in `world.chunks()`.
    world_map_tex: Option<egui::TextureHandle>,
    /// Whether the full-world map overlay (toggled with `M`) is currently shown.
    map_open: bool,
    /// Player-saved favorite viewpoints, one per number key 1–5. Loaded from and
    /// written back to `save.json`; mirrored into `SaveData` on every save.
    favorites: [Option<CameraSave>; crate::world_core::save::FAVORITE_SLOTS],
    /// A single-tap on a favorite key recalls; a double-tap saves. We can't tell
    /// the two apart until the double-tap window closes, so a press arms a pending
    /// recall here (slot, deadline in `elapsed_seconds`); a second press of the
    /// same slot before the deadline cancels it and saves instead. Resolved in
    /// `update`. See `on_favorite_key` / `resolve_pending_recall`.
    pending_recall: Option<(usize, f32)>,
    /// Brief on-screen text confirming a favorite was saved/recalled
    /// (message, `elapsed_seconds` at creation). Drawn by `draw_favorite_toast`.
    favorite_toast: Option<(String, f32)>,
    /// Whether the keyboard-shortcuts help overlay (toggled with `H`) is shown.
    show_help: bool,
    /// Handle to the background world-generation worker (native only). Polled each
    /// frame; dropped (detaching the thread) if the user leaves the loading screen.
    #[cfg(not(target_arch = "wasm32"))]
    world_gen_job: Option<WorldGenJob>,
    /// Result slot for the async world build (wasm only). The spawned future
    /// fetches the prebuilt base snapshot, builds the world, and parks the result
    /// here; `tick_create_world` polls it each frame so the loading screen keeps
    /// animating during the download.
    #[cfg(target_arch = "wasm32")]
    wasm_world_job: Option<std::rc::Rc<std::cell::RefCell<Option<anyhow::Result<WorldRuntime>>>>>,
    /// Live build fraction (`0.0..=1.0`) written by the cooperative web world
    /// build and read each frame to drive the loading bar.
    #[cfg(target_arch = "wasm32")]
    wasm_world_progress: Option<std::rc::Rc<std::cell::Cell<f32>>>,
    /// Base-world snapshot download started at launch (while the player is on the
    /// start menu) so a New Game can consume it immediately instead of waiting on
    /// the network. Persists for the app's lifetime and survives loading retries.
    #[cfg(target_arch = "wasm32")]
    wasm_base_prefetch: Option<std::rc::Rc<std::cell::RefCell<BasePrefetch>>>,
    thumbnail_renderer: Option<ThumbnailRenderer>,
    blur_pass: BlurPass,
    blur_capture_pending: bool,
    #[cfg(not(target_arch = "wasm32"))]
    benchmark: Option<benchmark::Benchmark>,
    /// CPU duration (ms) of the most recent `update()`, recorded by the event
    /// loop so `render()` can attribute total CPU-active time for benchmark mode.
    #[cfg(not(target_arch = "wasm32"))]
    update_cpu_ms: f32,
}

impl AppState {
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn new(
        window: &'static Window,
        debug_api_config: DebugApiConfig,
        _cursor_captured: bool,
        benchmark_path: Option<std::path::PathBuf>,
        instance: Option<String>,
    ) -> Result<Self> {
        let storage = create_storage(instance.as_deref());
        let captures_dir =
            crate::world_core::storage::instance_root(instance.as_deref()).join("captures");
        let config = GameConfig::load(&*storage);
        let save = SaveData::load(&*storage);
        let favorites = save.as_ref().map(|s| s.favorites).unwrap_or_default();

        let mut gpu = GpuContext::new(window).await?;

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

        // Benchmark mode: load the scripted flythrough and switch the surface to a
        // non-vsync present mode so we measure true render throughput, not refresh rate.
        let benchmark = match &benchmark_path {
            Some(path) => {
                gpu.enable_no_vsync();
                Some(benchmark::Benchmark::load(path)?)
            }
            None => None,
        };

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
            gpu.config.format,
            gpu.render_format,
            gpu.config.width,
            gpu.config.height,
        );

        let mut app = Self {
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
            last_autosave_seconds: 0.0,
            frame_index: 0,
            screenshot_pending: None,
            screenshot_to_clipboard: false,
            screenshot_toast: None,
            captures_dir,
            asset_watcher,
            audio: crate::audio::AudioSystem::new(),
            benchmark,
            update_cpu_ms: 0.0,
            egui_bridge,
            egui_pass,
            config_panel,
            settings_panel: SettingsPanel::new(),
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
            world_gen_progress: None,
            loading_started: None,
            loading_map_tex: None,
            loading_map_buf: Vec::new(),
            loading_map_done: 0,
            world_map_tex: None,
            map_open: false,
            favorites,
            pending_recall: None,
            favorite_toast: None,
            show_help: false,
            world_gen_job: None,
            thumbnail_renderer: None,
            blur_pass,
            blur_capture_pending: false,
        };

        // In benchmark mode, skip the menu and start loading the world immediately.
        #[cfg(not(target_arch = "wasm32"))]
        if app.benchmark.is_some() {
            app.begin_loading(false);
        }

        Ok(app)
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn new_web(window: &'static Window, _cursor_captured: bool) -> Result<Self> {
        let storage = create_storage(None);
        let config = GameConfig::load(&*storage);
        let save = SaveData::load(&*storage);
        let favorites = save.as_ref().map(|s| s.favorites).unwrap_or_default();

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
            gpu.config.format,
            gpu.render_format,
            gpu.config.width,
            gpu.config.height,
        );

        // Start downloading the base-world snapshot now, overlapping the network
        // with the time the player spends on the start menu, so clicking New Game
        // consumes the bytes immediately instead of waiting on the fetch. The
        // download is the same file regardless of `gen_key` (the query is only
        // cache-busting); the snapshot is validated against the live key later.
        let wasm_base_prefetch = {
            let gen_key = herbarium.generation_key(&config);
            let slot = std::rc::Rc::new(std::cell::RefCell::new(BasePrefetch::InFlight));
            let slot_for_task = std::rc::Rc::clone(&slot);
            wasm_bindgen_futures::spawn_local(async move {
                let bytes = crate::world_core::storage::fetch_base_world_async(gen_key).await;
                *slot_for_task.borrow_mut() = BasePrefetch::Ready(bytes);
            });
            Some(slot)
        };

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
            settings_panel: SettingsPanel::new(),
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
            world_gen_progress: None,
            loading_started: None,
            loading_map_tex: None,
            loading_map_buf: Vec::new(),
            loading_map_done: 0,
            world_map_tex: None,
            map_open: false,
            favorites,
            pending_recall: None,
            favorite_toast: None,
            show_help: false,
            wasm_world_job: None,
            wasm_world_progress: None,
            wasm_base_prefetch,
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

    /// Whether plain gameplay currently wants the cursor captured: we're
    /// `Playing` with nothing else (config panel, map, or help) claiming it. Both
    /// overlay toggles consult this before recapturing on close so dismissing one
    /// overlay never steals the cursor back while another is still open.
    fn gameplay_wants_cursor(&self) -> bool {
        matches!(self.screen, Screen::Playing)
            && !self.config_panel.is_visible()
            && !self.map_open
            && !self.show_help
    }

    /// Toggle the full-world map overlay (`M`). Releases the cursor while open and
    /// recaptures it on close — but only when gameplay actually wants the cursor
    /// (see `gameplay_wants_cursor`). The keyboard path already gates on the
    /// screen, but the debug API can toggle this from any state, so guard the
    /// recapture here too rather than locking the cursor on a menu/other overlay.
    fn toggle_map_overlay(&mut self) {
        self.map_open = !self.map_open;
        if self.map_open {
            self.release_cursor();
        } else if self.gameplay_wants_cursor() {
            self.capture_cursor();
        }
    }

    /// Toggle the keyboard-shortcuts help overlay. Like the map, it releases the
    /// cursor while open and recaptures it on close only when gameplay wants it.
    fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
        if self.show_help {
            self.release_cursor();
        } else if self.gameplay_wants_cursor() {
            self.capture_cursor();
        }
    }

    fn teleport_camera_to_map_position(&mut self, x: f32, z: f32) {
        let ground_y = self
            .world
            .as_ref()
            .map(|world| world.sample_height(x, z))
            .unwrap_or(self.camera.position.y - MIN_HEIGHT_ABOVE_GROUND);
        // Where the terrain dips below the water line, land on the water surface
        // instead of the seabed so the teleport never drops the camera underwater.
        let surface_y = ground_y.max(self.config.sea_level);
        self.camera.position = Vec3::new(x, surface_y + MIN_HEIGHT_ABOVE_GROUND, z);
        self.camera_controller.reset_inputs();
    }

    /// Handle a press of a favorite-position key (`slot` is 0-based; keys 1–5).
    /// A single tap recalls the slot, a double tap saves the current viewpoint
    /// into it. We can't distinguish the two until the double-tap window closes,
    /// so the first press arms a pending recall (resolved in `update`) and a
    /// second press of the same slot within the window cancels it and saves.
    fn on_favorite_key(&mut self, slot: usize) {
        if slot >= FAVORITE_SLOTS {
            return;
        }
        match self.pending_recall {
            Some((pending_slot, deadline))
                if pending_slot == slot && self.elapsed_seconds <= deadline =>
            {
                // Second tap on the same slot, still inside the window → save.
                self.pending_recall = None;
                self.save_favorite(slot);
            }
            other => {
                // A different slot was waiting: fire its recall now so a quick
                // 1-then-2 doesn't swallow the first jump. Then arm this slot.
                if let Some((pending_slot, _)) = other {
                    self.recall_favorite(pending_slot);
                }
                self.pending_recall =
                    Some((slot, self.elapsed_seconds + FAVORITE_DOUBLE_TAP_SECONDS));
            }
        }
    }

    /// Fire a pending single-tap recall once its double-tap window elapses with
    /// no second press. Called each frame from `update` while playing.
    fn resolve_pending_recall(&mut self) {
        if let Some((slot, deadline)) = self.pending_recall {
            if self.elapsed_seconds >= deadline {
                self.pending_recall = None;
                self.recall_favorite(slot);
            }
        }
    }

    /// Store the current camera position and view direction into `slot`, then
    /// persist so it survives a restart. Empty slots show on the world map.
    fn save_favorite(&mut self, slot: usize) {
        if slot >= FAVORITE_SLOTS {
            return;
        }
        self.favorites[slot] = Some(CameraSave {
            position: [
                self.camera.position.x,
                self.camera.position.y,
                self.camera.position.z,
            ],
            yaw: self.camera.yaw,
            pitch: self.camera.pitch,
        });
        self.set_favorite_toast(format!("Saved position {}", slot + 1));
        if let Err(e) = self.save_and_update() {
            log::warn!("failed to persist favorite position: {e}");
        }
    }

    /// Recall a saved viewpoint: restore position and look direction. The saved
    /// height is honoured but clamped above the current ground/water surface so a
    /// changed terrain (or a slot saved underwater) can't drop the camera under.
    fn recall_favorite(&mut self, slot: usize) {
        let Some(fav) = self.favorites.get(slot).and_then(|f| *f) else {
            return;
        };
        let [x, saved_y, z] = fav.position;
        let ground_y = self
            .world
            .as_ref()
            .map(|world| world.sample_height(x, z))
            .unwrap_or(saved_y - MIN_HEIGHT_ABOVE_GROUND);
        let min_y = ground_y.max(self.config.sea_level) + MIN_HEIGHT_ABOVE_GROUND;
        self.camera.position = Vec3::new(x, saved_y.max(min_y), z);
        self.camera.yaw = fav.yaw;
        self.camera.pitch = fav.pitch.clamp(-1.54, 1.54);
        self.camera_controller.reset_inputs();
        self.set_favorite_toast(format!("Position {}", slot + 1));
    }

    fn set_favorite_toast(&mut self, message: String) {
        self.favorite_toast = Some((message, self.elapsed_seconds));
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
        // Reset any leftover generation state from a previous build.
        self.world_gen_progress = None;
        self.loading_started = None;
        self.loading_map_tex = None;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.world_gen_job = None;
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.wasm_world_job = None;
            self.wasm_world_progress = None;
        }
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
                    // Fresh world: scan the generated terrain for a coastline and
                    // drop the player on land beside the sea, looking out over the
                    // water — far nicer than the fixed corner the world used to
                    // start in. Falls back to a fixed pose if there's no coast.
                    None => {
                        let hm = crate::world_core::heightmap::Heightmap::new(
                            self.config.world.seed,
                            self.config.heightmap.clone(),
                        );
                        match hm.find_coastal_spawn(self.config.sea_level) {
                            Some(s) => (
                                Vec3::new(s.x, s.ground + 3.0, s.z),
                                s.water_dir.1.atan2(s.water_dir.0),
                                -0.12,
                            ),
                            None => (Vec3::new(158.0, 72.0, -51.0), 4.0, -0.23),
                        }
                    }
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
                self.tick_create_world(resume);
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
                // Anchor the autosave cadence to when gameplay actually starts,
                // not app launch — `elapsed_seconds` also accrues in the menu and
                // during the (multi-second) load, which would otherwise trip an
                // autosave on the very first playing frame.
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.last_autosave_seconds = self.elapsed_seconds;
                }
                #[cfg(not(target_arch = "wasm32"))]
                let skip_capture = self.benchmark.is_some();
                #[cfg(target_arch = "wasm32")]
                let skip_capture = false;
                if !skip_capture {
                    self.capture_cursor();
                }
                self.loading_state = None;
                // Build the in-game map overlay (M key). Rather than reuse the
                // blocky one-cell-per-chunk loading map, resample the world's
                // continuous heightmap into a smooth, relief-shaded map. It's
                // static, so this one-time build is all the overlay needs.
                self.build_world_map_texture();
                // Release the generation-visualization resources now that we're
                // in-game; they're rebuilt fresh on the next load.
                self.world_gen_progress = None;
                self.loading_started = None;
                self.loading_map_tex = None;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.world_gen_job = None;
                }
            }
        }
    }

    /// `CreateWorld` phase. On native, spawn the heavy `WorldRuntime::generate` on
    /// a worker thread the first time through, then poll for its result on
    /// subsequent frames so the event loop keeps animating. On wasm (no threads)
    /// it runs inline — blocking is acceptable there.
    fn tick_create_world(&mut self, resume: bool) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            // First entry: kick off the worker.
            if self.world_gen_job.is_none() {
                let threads = std::thread::available_parallelism()
                    .map(|n| n.get())
                    .unwrap_or(4);
                let registry = self.loading_registry.take().unwrap_or_else(|| {
                    std::sync::Arc::new(
                        crate::world_core::herbarium::PlantRegistry::from_herbarium(
                            &self.herbarium,
                        ),
                    )
                });
                let config = self.config.clone();
                let save = if resume { self.save.clone() } else { None };
                // Storage handles aren't `Send`; read the persisted blobs here on
                // the main thread and hand the owned bytes to the worker. The
                // `world_base` cache lets a New Game skip regeneration; `gen_key`
                // keys it to the current generation rules so a stale cache is
                // rejected.
                let spread_bytes = self.storage.load_bytes("plants");
                let base_bytes = self.storage.load_bytes("world_base");
                let gen_key = self.herbarium.generation_key(&self.config);
                let progress = std::sync::Arc::new(crate::world_runtime::GenerationProgress::new());
                self.world_gen_progress = Some(std::sync::Arc::clone(&progress));
                self.loading_started = Some(Instant::now());

                let (tx, rx) = std::sync::mpsc::channel();
                let spawned = std::thread::Builder::new()
                    .name("world-gen".to_string())
                    .spawn(move || {
                        let result = WorldRuntime::generate(
                            config,
                            save,
                            threads,
                            registry,
                            spread_bytes,
                            base_bytes,
                            gen_key,
                            &progress,
                        );
                        // Ignore send errors: if the receiver was dropped the user
                        // left the loading screen and the result is no longer wanted.
                        let _ = tx.send(result);
                    });
                match spawned {
                    Ok(handle) => {
                        self.world_gen_job = Some(WorldGenJob {
                            result: rx,
                            _handle: handle,
                        });
                    }
                    Err(err) => {
                        // Thread creation can fail under resource limits; bail back
                        // to the menu instead of panicking.
                        log::error!("failed to spawn world-gen thread: {err}");
                        self.abort_loading();
                    }
                }
                return;
            }

            // Subsequent frames: poll the worker without blocking.
            let msg = self.world_gen_job.as_ref().map(|job| job.result.try_recv());
            match msg {
                Some(Ok(Ok(mut world))) => {
                    // A New Game that regenerated the base stages a snapshot to
                    // persist; write it here (off the generation thread, which
                    // can't touch the non-`Send` storage handle) so the next New
                    // Game loads it instead of regenerating.
                    if let Some(bytes) = world.take_pending_base_snapshot() {
                        if let Err(err) = self.storage.save_bytes("world_base", &bytes) {
                            log::warn!("failed to cache base world: {err}");
                        }
                    }
                    self.world = Some(world);
                    self.world_gen_job = None;
                    if let Some(s) = &mut self.loading_state {
                        s.phase = LoadingPhase::DispatchChunks;
                        s.progress = 0.30;
                    }
                }
                Some(Ok(Err(err))) => {
                    log::error!("failed to create world runtime: {err}");
                    self.abort_loading();
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    log::error!("world-gen worker thread terminated unexpectedly");
                    self.abort_loading();
                }
                Some(Err(std::sync::mpsc::TryRecvError::Empty)) => {
                    // Still generating: drive the progress bar from the live
                    // fraction (CreateWorld owns the 0.15..0.30 band; the map
                    // visualization shows the real per-chunk detail).
                    if let Some(p) = &self.world_gen_progress {
                        let frac = p.fraction();
                        if let Some(s) = &mut self.loading_state {
                            s.progress = 0.15 + frac * 0.15;
                        }
                    }
                }
                None => {}
            }
        }

        #[cfg(target_arch = "wasm32")]
        {
            // No threads on wasm. First entry: spawn an async task that downloads
            // the prebuilt base snapshot, then builds the world a step at a time,
            // yielding to the browser between steps so the loading screen keeps
            // animating instead of freezing during the decode + rebuild. A
            // validated download skips the multi-minute single-threaded
            // generation; a miss still falls back to it.
            if self.wasm_world_job.is_none() {
                let registry = self.loading_registry.take().unwrap_or_else(|| {
                    std::sync::Arc::new(
                        crate::world_core::herbarium::PlantRegistry::from_herbarium(
                            &self.herbarium,
                        ),
                    )
                });
                let config = self.config.clone();
                let save = if resume { self.save.clone() } else { None };
                let gen_key = self.herbarium.generation_key(&self.config);
                let prefetch = self.wasm_base_prefetch.clone();
                let slot: std::rc::Rc<std::cell::RefCell<Option<anyhow::Result<WorldRuntime>>>> =
                    std::rc::Rc::new(std::cell::RefCell::new(None));
                let progress = std::rc::Rc::new(std::cell::Cell::new(0.0f32));
                // The task holds only a Weak ref so leaving the loading screen
                // (`abort_loading` drops `wasm_world_job`) lets us cancel between
                // build steps.
                let weak = std::rc::Rc::downgrade(&slot);
                let progress_for_task = std::rc::Rc::clone(&progress);
                self.wasm_world_job = Some(slot);
                self.wasm_world_progress = Some(progress);
                self.loading_started = Some(Instant::now());

                wasm_bindgen_futures::spawn_local(async move {
                    // Only a New Game downloads the base; a resume rebuilds from
                    // its own seed without it (matching the native path).
                    let base_bytes = if save.is_none() {
                        // Consume the launch-time prefetch, cloning so a loading
                        // retry can reuse it. If it's still in flight, yield until
                        // it lands (the loading screen keeps animating). With no
                        // prefetch slot, fetch inline as a fallback.
                        match prefetch {
                            Some(slot) => loop {
                                if let BasePrefetch::Ready(bytes) = &*slot.borrow() {
                                    break bytes.clone();
                                }
                                yield_to_browser().await;
                            },
                            None => {
                                crate::world_core::storage::fetch_base_world_async(gen_key).await
                            }
                        }
                    } else {
                        None
                    };
                    let mut builder = crate::world_runtime::WebWorldBuilder::new(
                        &config,
                        save.as_ref(),
                        registry,
                        base_bytes,
                        gen_key,
                    );
                    loop {
                        // Re-acquire the slot each step so leaving the loading
                        // screen (which drops the only other strong ref) cancels
                        // the build before the next step runs.
                        let Some(slot) = weak.upgrade() else {
                            return;
                        };
                        match builder.step() {
                            Ok(crate::world_runtime::BuildStep::Done(world)) => {
                                *slot.borrow_mut() = Some(Ok(*world));
                                return;
                            }
                            Ok(crate::world_runtime::BuildStep::Working(frac)) => {
                                progress_for_task.set(frac);
                                // Drop the strong ref before yielding so a cancel
                                // mid-yield isn't blocked by us holding it.
                                drop(slot);
                                yield_to_browser().await;
                            }
                            Err(err) => {
                                *slot.borrow_mut() = Some(Err(err));
                                return;
                            }
                        }
                    }
                });
                return;
            }

            // Subsequent frames: drive the loading bar from the live build
            // fraction (CreateWorld owns the 0.15..0.30 band), then poll the slot
            // for the finished runtime without blocking.
            if let (Some(p), Some(s)) = (&self.wasm_world_progress, &mut self.loading_state) {
                s.progress = 0.15 + p.get() * 0.15;
            }
            let done = self
                .wasm_world_job
                .as_ref()
                .and_then(|slot| slot.borrow_mut().take());
            match done {
                Some(Ok(world)) => {
                    self.world = Some(world);
                    self.wasm_world_job = None;
                    self.wasm_world_progress = None;
                    if let Some(s) = &mut self.loading_state {
                        s.phase = LoadingPhase::DispatchChunks;
                        s.progress = 0.30;
                    }
                }
                Some(Err(err)) => {
                    log::error!("failed to create world runtime: {err}");
                    self.abort_loading();
                }
                None => {}
            }
        }
    }

    /// Rebuild the 256×256 biome map texture from the worker's per-chunk progress
    /// buffer. Each cell is `cell_color(byte)`; ungenerated cells are transparent
    /// so the unfilled map reads as empty over the background. The RGBA scratch is
    /// reused and the GPU upload is skipped on frames where no new chunk finished,
    /// so this stays cheap across the multi-second build.
    fn update_loading_map_texture(&mut self) {
        use crate::world_core::chunk::WORLD_SIZE_CHUNKS;
        use crate::world_runtime::gen_progress::cell_color;

        let Some(progress) = &self.world_gen_progress else {
            return;
        };
        let done = progress.done();
        // Nothing new finished since the last upload (and the texture already
        // exists): keep the current one rather than re-uploading an identical map.
        if self.loading_map_tex.is_some() && done == self.loading_map_done {
            return;
        }

        let n = WORLD_SIZE_CHUNKS as usize;
        self.loading_map_buf.resize(n * n * 4, 0);
        for idx in 0..(n * n) {
            let color = cell_color(progress.cell(idx));
            let off = idx * 4;
            self.loading_map_buf[off..off + 4].copy_from_slice(&color);
        }
        self.loading_map_done = done;

        let image = egui::ColorImage::from_rgba_unmultiplied([n, n], &self.loading_map_buf);
        match &mut self.loading_map_tex {
            Some(tex) => tex.set(image, egui::TextureOptions::NEAREST),
            None => {
                let tex = self.egui_bridge.ctx().load_texture(
                    "loading-world-map",
                    image,
                    egui::TextureOptions::NEAREST,
                );
                self.loading_map_tex = Some(tex);
            }
        }
    }

    /// Build the static, relief-shaded world map shown by the `M`-key overlay.
    /// Resamples the world's continuous heightmap at high resolution (smooth
    /// coastlines, hypsometric elevation tints, flat sea, hillshade relief)
    /// instead of reusing the blocky per-chunk loading map. Falls back to the
    /// loading map if the world somehow isn't ready yet.
    fn build_world_map_texture(&mut self) {
        let res = crate::world_runtime::WORLD_MAP_RES;
        let Some(world) = &self.world else {
            self.update_loading_map_texture();
            self.world_map_tex = self.loading_map_tex.take();
            return;
        };
        let buf = world.render_world_map(res);
        let image = egui::ColorImage::from_rgba_unmultiplied([res, res], &buf);
        self.world_map_tex = Some(self.egui_bridge.ctx().load_texture(
            "world-map",
            image,
            egui::TextureOptions::LINEAR,
        ));
    }

    /// Bail out of loading back to the start menu, releasing generation state.
    fn abort_loading(&mut self) {
        self.screen = Screen::StartMenu;
        self.loading_state = None;
        self.world_gen_progress = None;
        self.loading_started = None;
        self.loading_map_tex = None;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.world_gen_job = None;
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.wasm_world_job = None;
            self.wasm_world_progress = None;
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

    #[cfg(not(target_arch = "wasm32"))]
    fn benchmark_finished(&self) -> bool {
        self.benchmark
            .as_ref()
            .map(|b| b.is_finished())
            .unwrap_or(false)
    }

    fn update(&mut self) {
        self.frame_index = self.frame_index.saturating_add(1);

        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;

        self.frame_time_ms = self.frame_time_ms * 0.94 + (dt * 1000.0) * 0.06;
        self.elapsed_seconds += dt;

        // Ambient sound plays in the world and on the start menu (so volume
        // changes in Settings are audible while adjusting them); it stays paused
        // on the other non-playing screens (loading, herbarium, plant editor).
        #[cfg(not(target_arch = "wasm32"))]
        if self.is_on_menu() {
            if let Some(audio) = self.audio.as_mut() {
                audio.update_menu(dt, &self.config.audio);
            }
        } else if self.is_loading() || self.is_on_herbarium() || self.is_on_editor() {
            if let Some(audio) = &self.audio {
                audio.pause();
            }
        }

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

        // Playing: fire a pending favorite recall once its double-tap window closes.
        self.resolve_pending_recall();

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

        // In benchmark mode, drive the camera from the script on a fixed timestep
        // so the simulated workload is identical across runs; the real wall-clock
        // `dt` is what we record. Otherwise sim_dt == dt and behavior is unchanged.
        #[cfg(not(target_arch = "wasm32"))]
        let sim_dt = if let Some(mut bench) = self.benchmark.take() {
            bench.tick(dt * 1000.0, &mut self.camera, &mut self.camera_controller);
            let fixed = bench.fixed_dt();
            self.benchmark = Some(bench);
            fixed
        } else {
            dt
        };
        #[cfg(target_arch = "wasm32")]
        let sim_dt = dt;

        self.camera_controller.update_camera(
            sim_dt,
            &mut self.camera,
            self.focused && self.cursor_captured,
        );

        let world = self.world.as_mut().unwrap();

        clamp_camera_to_terrain(&mut self.camera, world.chunks());

        world.update(sim_dt, self.camera.position);
        self.world_renderer
            .sync_chunks(&self.gpu.device, &self.gpu.queue, world.chunks());

        // Drive the ambient sound mix from the camera's surroundings.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(audio) = self.audio.as_mut() {
            audio.update(
                sim_dt,
                self.camera.position,
                world.chunks(),
                self.config.sea_level,
                &self.config.audio,
            );
        }

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
            lighting.sun_direction,
            None,
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
            1000.0 / self.frame_time_ms.max(0.01),
            stats.world_population,
            self.gpu.config.width as f32,
            self.gpu.config.height as f32,
        );
        self.world_renderer.update_minimap(
            &self.gpu.queue,
            &self.gpu.device,
            sim_dt,
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

        // Periodic autosave so an unexpected exit (crash, power loss) between
        // explicit saves doesn't lose progress. Best-effort and cheap — only the
        // camera metadata and the plant spread delta are written, not the
        // regenerable base world. Skipped in benchmark mode so the scripted
        // flythrough isn't perturbed by disk I/O.
        #[cfg(not(target_arch = "wasm32"))]
        if self.benchmark.is_none()
            && self.elapsed_seconds - self.last_autosave_seconds >= AUTOSAVE_INTERVAL_SECONDS
        {
            self.last_autosave_seconds = self.elapsed_seconds;
            let _ = self.save_and_update();
        }

        #[cfg(not(target_arch = "wasm32"))]
        self.publish_telemetry_if_due(&stats);
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
            light_dir,
            // Studio scene: never apply underwater murk regardless of camera height.
            Some(0.0),
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
                            // Up/Down zoom the orbit camera (mirrors the mouse
                            // wheel); a press is one chunky zoom step.
                            MoveKey::Up if pressed => editor.on_scroll(4.0),
                            MoveKey::Down if pressed => editor.on_scroll(-4.0),
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
            light_dir,
            None,
        );
        self.world_renderer
            .update_material(&self.gpu.queue, light_dir, ambient, &palette);
    }

    fn render(&mut self) -> Result<(), SurfaceError> {
        // Benchmark timing: the acquire call blocks while the GPU catches up, so
        // its duration is the per-frame GPU-bound wait; the span from here to
        // submit is the render-thread CPU cost.
        #[cfg(not(target_arch = "wasm32"))]
        let t_acquire = Instant::now();
        let output = self.gpu.surface.get_current_texture()?;
        #[cfg(not(target_arch = "wasm32"))]
        let (gpu_wait_ms, t_encode) = (t_acquire.elapsed().as_secs_f32() * 1000.0, Instant::now());
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

        // Shadow depth pass: render the scene from the sun's point of view into
        // the shadow map before the main color pass. Only needed when the 3D
        // world (not the sky-only menu/loading/herbarium screens) is drawn.
        let renders_world =
            is_editor || self.blur_capture_pending || !(is_menu || is_loading || is_herbarium);
        if renders_world {
            self.world_renderer.render_shadows(&mut encoder);
        }

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
            self.blur_capture_pending = false;
        }

        // egui overlay pass (renders on top of 3D scene)
        {
            self.ui_registry.clear();
            #[allow(unused_mut)]
            let mut show_egui = is_menu
                || is_loading
                || is_herbarium
                || self.config_panel.is_visible()
                || is_editor
                || self.map_open
                || self.show_help;
            // The confirmation toast renders during plain gameplay, when egui
            // would otherwise be skipped, so keep the pass alive while it shows.
            #[cfg(not(target_arch = "wasm32"))]
            {
                show_egui = show_egui || self.screenshot_toast.is_some();
            }
            // The favorite save/recall toast also shows during plain gameplay
            // (both native and web), so keep the egui pass alive while it lingers.
            show_egui = show_egui || self.favorite_toast.is_some();
            if show_egui {
                // Refresh the biome map texture from the worker's live progress
                // before egui runs (needs `ctx` + `&mut self`, which the run
                // closure can't also hold).
                if is_loading {
                    self.update_loading_map_texture();
                }
                let raw_input = self.egui_bridge.take_raw_input();
                let mut menu_action = None;
                let mut settings_closed = false;
                let loading_progress = self
                    .loading_state
                    .as_ref()
                    .map(|s| s.progress)
                    .unwrap_or(0.0);
                let loading_elapsed = self
                    .loading_started
                    .map(|t| t.elapsed().as_secs_f32())
                    .unwrap_or(0.0);
                let mut map_teleport_target = None;
                let full_output = self.egui_bridge.ctx().run(raw_input, |ctx| {
                    match self.screen {
                        Screen::StartMenu => {
                            // Capture before drawing: when the settings dialog is
                            // open it's modal, so start-menu buttons must not act
                            // (egui z-orders mouse clicks, but the debug API's
                            // consume_click bypasses that, so gate here too).
                            let settings_open = self.settings_panel.is_open();
                            let menu = self.start_menu.ui(ctx, &mut self.ui_registry);
                            if !settings_open {
                                menu_action = menu;
                            }
                            settings_closed = self.settings_panel.ui(
                                ctx,
                                &mut self.ui_registry,
                                &mut self.config.audio,
                            );
                        }
                        Screen::Loading => {
                            render_loading_ui(
                                ctx,
                                loading_progress,
                                self.world_gen_progress.as_deref(),
                                loading_elapsed,
                                self.loading_map_tex.as_ref(),
                            );
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
                            if self.map_open {
                                if let Some((x, z)) = render_map_ui(
                                    ctx,
                                    self.camera.position.x,
                                    self.camera.position.z,
                                    self.camera.yaw,
                                    self.world_map_tex.as_ref(),
                                    &self.favorites,
                                ) {
                                    map_teleport_target = Some((x, z));
                                }
                            }
                            if self.show_help {
                                render_help_ui(ctx);
                            }
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
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    if let Some(toast) = &self.screenshot_toast {
                        draw_screenshot_toast(ctx, toast, self.elapsed_seconds);
                    }
                    if let Some((msg, created_at)) = &self.favorite_toast {
                        draw_favorite_toast(ctx, msg, *created_at, self.elapsed_seconds);
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

                if let Some((x, z)) = map_teleport_target {
                    self.teleport_camera_to_map_position(x, z);
                }

                // Persist audio/settings changes once the dialog is dismissed.
                if settings_closed {
                    self.persist_config();
                }
            }
        }

        // Drop the confirmation toast (and free its egui texture) once it has
        // finished sliding out.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(toast) = &self.screenshot_toast {
            if self.elapsed_seconds - toast.created_at
                >= SCREENSHOT_TOAST_HOLD_S + SCREENSHOT_TOAST_SLIDE_S
            {
                self.screenshot_toast = None;
            }
        }

        if let Some((_, created_at)) = &self.favorite_toast {
            if self.elapsed_seconds - *created_at >= FAVORITE_TOAST_SECONDS {
                self.favorite_toast = None;
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(command_id) = self.screenshot_pending.take() {
            self.handle_screenshot(command_id, &output.texture, encoder);
            output.present();
            return Ok(());
        }

        self.gpu.queue.submit(Some(encoder.finish()));

        // Record the frame's GPU-wait and CPU-active times (CPU = update + encode).
        #[cfg(not(target_arch = "wasm32"))]
        {
            let cpu_ms = self.update_cpu_ms + t_encode.elapsed().as_secs_f32() * 1000.0;
            if let Some(bench) = &mut self.benchmark {
                bench.record_frame_timing(gpu_wait_ms, cpu_ms);
            }
        }

        output.present();
        Ok(())
    }

    /// Write the current `GameConfig` to storage (`config.json`). Used to make
    /// settings changes (e.g. audio volume) survive a restart.
    fn persist_config(&self) {
        match serde_json::to_string_pretty(&self.config) {
            Ok(json) => {
                if let Err(e) = self.storage.save("config", &json) {
                    log::warn!("failed to save config: {e}");
                }
            }
            Err(e) => log::warn!("failed to serialize config: {e}"),
        }
    }

    fn save_game(&self) {
        let Some(world) = &self.world else { return };
        let save = self.build_save_data(world);
        if let Err(e) = save.save(&*self.storage) {
            log::warn!("failed to save game state: {e}");
        }
        if let Err(e) = world.save_plants(&*self.storage) {
            log::warn!("failed to save plant state: {e}");
        }
    }

    /// Save to storage and update the in-memory save (for mid-session resume).
    /// Returns `Ok` only when both the metadata and the plant state were written.
    fn save_and_update(&mut self) -> anyhow::Result<()> {
        let Some(world) = &self.world else {
            return Err(anyhow::anyhow!("no world loaded"));
        };
        let save = self.build_save_data(world);
        if let Err(e) = save.save(&*self.storage) {
            log::warn!("failed to save game state: {e}");
            return Err(e);
        }
        if let Err(e) = world.save_plants(&*self.storage) {
            log::warn!("failed to save plant state: {e}");
            // The new metadata is already on disk but the plant blob isn't, so
            // restore the previous metadata (best-effort) to keep the on-disk
            // save.json / plants.bin pair consistent.
            if let Some(prev) = &self.save {
                let _ = prev.save(&*self.storage);
            }
            return Err(e);
        }
        // Only advance the in-memory resume point once both writes succeeded.
        self.save = Some(save);
        Ok(())
    }

    fn build_save_data(&self, world: &WorldRuntime) -> SaveData {
        SaveData {
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
                seed: world.seed(),
                hour: world.hour(),
                day_speed: world.day_speed(),
                total_hours: world.total_hours(),
            },
            favorites: self.favorites,
        }
    }
}

/// The "world being born" loading screen: a top-down biome map that paints
/// itself in as generation sweeps through the canonical chunks, with a glowing
/// frontier, themed status text, and a live `% • elapsed • chunks` readout. The
/// map texture (`map_tex`) is updated each frame by `update_loading_map_texture`;
/// `gen` carries the real per-chunk progress (None before/after the build).
/// A brief on-screen confirmation that a screenshot was copied to the clipboard:
/// a thumbnail that rests in the top-right corner, then slides out to the right.
#[cfg(not(target_arch = "wasm32"))]
struct ScreenshotToast {
    texture: egui::TextureHandle,
    /// `elapsed_seconds` at capture time; drives the hold + slide animation.
    created_at: f32,
    /// Thumbnail width / height, so the toast keeps the frame's aspect ratio.
    aspect: f32,
}

/// How long the toast holds in place, then how long it takes to slide off-screen.
#[cfg(not(target_arch = "wasm32"))]
const SCREENSHOT_TOAST_HOLD_S: f32 = 2.0;
#[cfg(not(target_arch = "wasm32"))]
const SCREENSHOT_TOAST_SLIDE_S: f32 = 0.45;

/// Draw the screenshot-confirmation toast: a bordered thumbnail pinned to the
/// top-right that, after a hold, eases off the right edge while fading out.
#[cfg(not(target_arch = "wasm32"))]
fn draw_screenshot_toast(ctx: &egui::Context, toast: &ScreenshotToast, elapsed_s: f32) {
    use egui::{Color32, CornerRadius, Id, LayerId, Order, Rect, Stroke, StrokeKind};

    let age = elapsed_s - toast.created_at;
    if !(0.0..SCREENSHOT_TOAST_HOLD_S + SCREENSHOT_TOAST_SLIDE_S).contains(&age) {
        return;
    }

    let screen = ctx.content_rect();
    let margin = 16.0;
    let width = (screen.width() * 0.22).clamp(160.0, 300.0);
    let height = width / toast.aspect.max(0.01);

    // Hold, then slide out with an ease-in cubic so it accelerates off-screen.
    let slide_t = ((age - SCREENSHOT_TOAST_HOLD_S) / SCREENSHOT_TOAST_SLIDE_S).clamp(0.0, 1.0);
    let ease = slide_t * slide_t * slide_t;
    let resting_x = screen.right() - margin - width;
    let x = resting_x + ease * (width + margin + 8.0);
    let rect = Rect::from_min_size(
        egui::pos2(x, screen.top() + margin),
        egui::vec2(width, height),
    );

    let fade = 1.0 - ease;
    let a = |v: f32| (v * fade).clamp(0.0, 255.0) as u8;

    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("screenshot-toast")));
    painter.rect_filled(
        rect.expand(3.0),
        CornerRadius::same(6),
        Color32::from_black_alpha(a(150.0)),
    );
    painter.image(
        toast.texture.id(),
        rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::from_white_alpha(a(255.0)),
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(6),
        Stroke::new(1.5, Color32::from_white_alpha(a(210.0))),
        StrokeKind::Inside,
    );
    // Keep frames flowing so the animation runs even if nothing else is dirty.
    ctx.request_repaint();
}

/// Draw the favorite-position confirmation: a small pill of text centered near
/// the bottom of the screen that fades out over its final third of life.
fn draw_favorite_toast(ctx: &egui::Context, message: &str, created_at: f32, elapsed_s: f32) {
    use egui::{Align2, Color32, CornerRadius, FontId, Id, LayerId, Order};

    let age = elapsed_s - created_at;
    if !(0.0..FAVORITE_TOAST_SECONDS).contains(&age) {
        return;
    }
    let fade = (1.0 - (age / FAVORITE_TOAST_SECONDS - 0.66) / 0.34).clamp(0.0, 1.0);
    let a = |v: f32| (v * fade).clamp(0.0, 255.0) as u8;

    let screen = ctx.content_rect();
    let center = egui::pos2(screen.center().x, screen.bottom() - 96.0);
    let font = FontId::proportional(20.0);
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("favorite-toast")));
    let galley = painter.layout_no_wrap(message.to_string(), font.clone(), Color32::WHITE);
    let pad = egui::vec2(16.0, 8.0);
    let bg = egui::Rect::from_center_size(center, galley.size() + pad * 2.0);
    painter.rect_filled(
        bg,
        CornerRadius::same(8),
        Color32::from_black_alpha(a(170.0)),
    );
    painter.text(
        center,
        Align2::CENTER_CENTER,
        message,
        font,
        Color32::from_rgba_unmultiplied(255, 230, 150, a(255.0)),
    );
    ctx.request_repaint();
}

fn render_loading_ui(
    ctx: &egui::Context,
    progress: f32,
    gen: Option<&crate::world_runtime::GenerationProgress>,
    elapsed_s: f32,
    map_tex: Option<&egui::TextureHandle>,
) {
    use egui::{Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind};

    let (done, total, gen_frac) = match gen {
        Some(g) => (g.done(), g.total(), g.fraction()),
        None => (0, 0, progress),
    };
    // The bar and status track generation while it runs, then ride the (fast)
    // post-gen sync phases to 100%.
    let frac = if gen.is_some() && gen_frac < 1.0 {
        gen_frac
    } else {
        progress.max(gen_frac)
    };

    let message = match frac {
        p if p < 0.05 => "Preparing the soil...",
        p if p < 0.30 => "Planting seeds...",
        p if p < 0.60 => "Growing forests...",
        p if p < 0.85 => "Carving rivers...",
        p if p < 0.999 => "Welcoming wildlife...",
        _ => "World ready!",
    };

    // Gentle pulse for the frontier glow (no per-cell timestamps needed).
    let pulse = 0.5 + 0.5 * (elapsed_s * 3.5).sin();

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(Color32::from_black_alpha(150)))
        .show(ctx, |ui| {
            let available = ui.available_size();
            // Square map sized to the smaller viewport dimension, leaving room
            // for the title above and the status/readout/bar below.
            let map_side = (available.x.min(available.y) * 0.55).clamp(180.0, 560.0);
            let top_pad = ((available.y - map_side) * 0.5 - 70.0).max(20.0);
            ui.add_space(top_pad);

            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Breathing life into a new world")
                        .size(26.0)
                        .strong()
                        .color(Color32::from_rgb(232, 240, 247)),
                );
                ui.add_space(16.0);

                let (rect, _resp) =
                    ui.allocate_exact_size(egui::vec2(map_side, map_side), Sense::hover());
                if ui.is_rect_visible(rect) {
                    let painter = ui.painter_at(rect);
                    // Backdrop behind the (partly transparent) map.
                    painter.rect_filled(
                        rect,
                        CornerRadius::same(8),
                        Color32::from_black_alpha(130),
                    );

                    if let Some(tex) = map_tex {
                        painter.image(
                            tex.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }

                    // Glowing progress line at the completed-fraction row. The
                    // rayon fan-out fills the map in parallel bands rather than a
                    // clean top-to-bottom sweep, so this is a progress indicator,
                    // not a literal generation frontier in map space.
                    if gen.is_some() && gen_frac > 0.0 && gen_frac < 1.0 {
                        let y = rect.top() + rect.height() * gen_frac;
                        let glow_h = 5.0 + 7.0 * pulse;
                        let glow = Rect::from_min_max(
                            egui::pos2(rect.left(), (y - glow_h).max(rect.top())),
                            egui::pos2(rect.right(), (y + glow_h).min(rect.bottom())),
                        );
                        painter.rect_filled(
                            glow,
                            CornerRadius::ZERO,
                            Color32::from_rgba_unmultiplied(
                                150,
                                205,
                                255,
                                (50.0 + 70.0 * pulse) as u8,
                            ),
                        );
                        painter.hline(
                            rect.x_range(),
                            y,
                            Stroke::new(1.5, Color32::from_rgba_unmultiplied(220, 240, 255, 210)),
                        );
                    }

                    // Crisp frame around the map.
                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(8),
                        Stroke::new(1.0, Color32::from_white_alpha(36)),
                        StrokeKind::Inside,
                    );
                }

                ui.add_space(18.0);
                ui.label(
                    RichText::new(message)
                        .size(20.0)
                        .color(Color32::from_rgb(220, 230, 240)),
                );
                ui.add_space(6.0);

                let pct = (frac * 100.0).round() as i32;
                let readout = if total > 0 {
                    format!("{pct}%   •   {elapsed_s:.0}s   •   {done} / {total} chunks")
                } else {
                    format!("{pct}%")
                };
                ui.label(
                    RichText::new(readout)
                        .font(FontId::monospace(13.0))
                        .color(Color32::from_white_alpha(170)),
                );
                ui.add_space(12.0);
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(map_side)
                        .animate(true),
                );
            });
        });
}

/// The full-world map overlay (toggled with `M`): a dimmed modal showing the
/// retained biome map of every canonical chunk, with the player drawn as an arrow
/// at their wrapped world position pointing along the camera heading. Right-click
/// on the map teleports the player to that world position; `M`/`Esc` closes it
/// (handled by the event loop).
fn render_map_ui(
    ctx: &egui::Context,
    cam_x: f32,
    cam_z: f32,
    cam_yaw: f32,
    map_tex: Option<&egui::TextureHandle>,
    favorites: &[Option<CameraSave>; FAVORITE_SLOTS],
) -> Option<(f32, f32)> {
    use crate::world_core::chunk::{CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS};
    use egui::{
        epaint, Align2, Color32, CornerRadius, CursorIcon, FontId, RichText, Sense, Shape, Stroke,
        StrokeKind,
    };

    let world_size_m = WORLD_SIZE_CHUNKS as f32 * CHUNK_SIZE_METERS;
    let mut teleport_target = None;

    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(Color32::from_black_alpha(190)))
        .show(ctx, |ui| {
            let available = ui.available_size();
            // Square map (the world is square) at ~80% of the smaller viewport
            // dimension, leaving room for the title above and the hint below. The
            // final `.min(avail_min)` keeps the 200px floor from overflowing a tiny
            // window/viewport, which would clip the map and overlap the chrome.
            let avail_min = available.x.min(available.y);
            let map_side = (avail_min * 0.8).clamp(200.0, 900.0).min(avail_min);
            let top_pad = ((available.y - map_side) * 0.5 - 48.0).max(16.0);
            ui.add_space(top_pad);

            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("World Map")
                        .size(26.0)
                        .strong()
                        .color(Color32::from_rgb(232, 240, 247)),
                );
                ui.add_space(12.0);

                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(map_side, map_side), Sense::click());
                let resp = resp.on_hover_cursor(CursorIcon::Crosshair);
                if resp.secondary_clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let u = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
                        let v = ((pos.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
                        teleport_target = Some(map_uv_to_world_position(u, v));
                    }
                }
                if ui.is_rect_visible(rect) {
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(
                        rect,
                        CornerRadius::same(8),
                        Color32::from_black_alpha(150),
                    );

                    match map_tex {
                        Some(tex) => {
                            // The texture is laid out world +x → column (texU) and
                            // world +z → row (texV). The compass and minimap put
                            // North (world +x) up and East (world +z) right, so the
                            // map must be transposed/flipped to match: screen-up =
                            // +x, screen-right = +z. egui's `image()` only does
                            // axis-aligned UV mapping, so we draw a mesh and assign
                            // each rect corner the UV that lands the right world cell
                            // there (e.g. top-left = max-x/min-z → uv (1, 0)).
                            let mut mesh = egui::Mesh::with_texture(tex.id());
                            let v = |pos: egui::Pos2, uv: egui::Pos2| epaint::Vertex {
                                pos,
                                uv,
                                color: Color32::WHITE,
                            };
                            mesh.vertices.push(v(rect.left_top(), egui::pos2(1.0, 0.0)));
                            mesh.vertices
                                .push(v(rect.right_top(), egui::pos2(1.0, 1.0)));
                            mesh.vertices
                                .push(v(rect.left_bottom(), egui::pos2(0.0, 0.0)));
                            mesh.vertices
                                .push(v(rect.right_bottom(), egui::pos2(0.0, 1.0)));
                            mesh.indices.extend_from_slice(&[0, 1, 2, 2, 1, 3]);
                            painter.add(Shape::mesh(mesh));
                        }
                        None => {
                            painter.text(
                                rect.center(),
                                Align2::CENTER_CENTER,
                                "Map unavailable",
                                FontId::proportional(18.0),
                                Color32::from_white_alpha(160),
                            );
                        }
                    }

                    // Saved favorite positions: numbered gold pins, drawn under the
                    // player arrow and visually distinct from it. Same world→screen
                    // mapping as the player marker. Display-only — recall is by key.
                    for (i, fav) in favorites.iter().enumerate() {
                        let Some(fav) = fav else { continue };
                        let [fx, _, fz] = fav.position;
                        let fu = fz.rem_euclid(world_size_m) / world_size_m;
                        let fv = 1.0 - fx.rem_euclid(world_size_m) / world_size_m;
                        let c = egui::pos2(
                            rect.left() + fu * rect.width(),
                            rect.top() + fv * rect.height(),
                        );
                        let r = (map_side * 0.016).clamp(7.0, 14.0);
                        painter.circle(
                            c,
                            r,
                            Color32::from_rgb(255, 205, 64),
                            Stroke::new(2.0, Color32::from_black_alpha(220)),
                        );
                        painter.text(
                            c,
                            Align2::CENTER_CENTER,
                            format!("{}", i + 1),
                            FontId::proportional(r * 1.25),
                            Color32::from_rgb(40, 30, 0),
                        );
                    }

                    // Player marker: an arrow at the wrapped world position pointing
                    // along the camera heading. Matching the compass/minimap, North
                    // (world +x) is up and East (world +z) is right: screen-right
                    // tracks +z, screen-down tracks -x.
                    let u = cam_z.rem_euclid(world_size_m) / world_size_m;
                    let v = 1.0 - cam_x.rem_euclid(world_size_m) / world_size_m;
                    let center = egui::pos2(
                        rect.left() + u * rect.width(),
                        rect.top() + v * rect.height(),
                    );
                    // Camera horizontal forward is (cos yaw, sin yaw) in world (x, z);
                    // mapped to screen that is (sin yaw, -cos yaw).
                    let dir = egui::vec2(cam_yaw.sin(), -cam_yaw.cos());
                    let perp = egui::vec2(-dir.y, dir.x);
                    let s = (map_side * 0.018).clamp(7.0, 16.0);
                    let tip = center + dir * s;
                    let back = center - dir * (s * 0.55);
                    let left = back + perp * (s * 0.7);
                    let right = back - perp * (s * 0.7);
                    // Bright fill with a dark outline so the marker reads over any
                    // biome color underneath it.
                    painter.add(Shape::convex_polygon(
                        vec![tip, left, right],
                        Color32::from_rgb(255, 64, 64),
                        Stroke::new(2.0, Color32::from_black_alpha(220)),
                    ));

                    if resp.hovered() {
                        if let Some(pos) = resp.interact_pointer_pos() {
                            let r = (map_side * 0.012).clamp(5.0, 11.0);
                            let stroke = Stroke::new(1.5, Color32::from_rgb(245, 248, 250));
                            let shadow = Stroke::new(3.0, Color32::from_black_alpha(180));
                            let h0 = egui::pos2(pos.x - r, pos.y);
                            let h1 = egui::pos2(pos.x + r, pos.y);
                            let v0 = egui::pos2(pos.x, pos.y - r);
                            let v1 = egui::pos2(pos.x, pos.y + r);
                            painter.circle_stroke(pos, r * 0.62, shadow);
                            painter.line_segment([h0, h1], shadow);
                            painter.line_segment([v0, v1], shadow);
                            painter.circle_stroke(pos, r * 0.62, stroke);
                            painter.line_segment([h0, h1], stroke);
                            painter.line_segment([v0, v1], stroke);
                        }
                    }

                    painter.rect_stroke(
                        rect,
                        CornerRadius::same(8),
                        Stroke::new(1.0, Color32::from_white_alpha(40)),
                        StrokeKind::Inside,
                    );
                }

                ui.add_space(14.0);
                ui.label(
                    RichText::new(
                        "Right-click to teleport   •   1–5 recall, double-tap to save   •   M / Esc to close",
                    )
                        .font(FontId::monospace(13.0))
                        .color(Color32::from_white_alpha(150)),
                );
            });
        });

    teleport_target
}

/// Modal overlay listing the keyboard shortcuts, toggled with `H`. Mirrors the
/// map overlay's dimmed full-screen frame so the two read as the same family of
/// pause-style overlays.
fn render_help_ui(ctx: &egui::Context) {
    use egui::{Color32, FontId, Grid, RichText};

    // (key, description) pairs grouped into titled sections. Kept in sync by hand
    // with the shortcut handlers in `app/event_loop.rs` and `camera.rs`.
    let sections: &[(&str, &[(&str, &str)])] = &[
        (
            "Navigation",
            &[
                ("W / \u{2191}", "Move forward"),
                ("S / \u{2193}", "Move backward"),
                ("A / \u{2190}", "Move left"),
                ("D / \u{2192}", "Move right"),
                ("Space", "Move up"),
                ("Shift", "Move down"),
                ("Ctrl", "Hold to move faster"),
                ("Mouse", "Look around"),
            ],
        ),
        (
            "World",
            &[
                ("M", "Toggle world map (right-click map to teleport)"),
                ("1 - 5", "Recall a favorite position (double-tap to save)"),
                ("F1", "Toggle config panel"),
                ("P", "Screenshot to clipboard"),
            ],
        ),
        (
            "General",
            &[
                ("H", "Toggle this help screen"),
                ("Esc", "Close overlay / return to menu"),
            ],
        ),
    ];

    let heading = Color32::from_rgb(232, 240, 247);
    let key_color = Color32::from_rgb(255, 224, 130);
    let desc_color = Color32::from_white_alpha(210);

    // Full-screen dim behind the card, matching the map overlay's frame so the
    // two overlays read as the same family.
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(Color32::from_black_alpha(190)))
        .show(ctx, |_ui| {});

    // The shortcuts sit in a centered, auto-sized card. A `Window` keeps the
    // two-column grids tidy (left-aligned within the card) instead of letting
    // them sprawl across the full viewport width.
    egui::Window::new("Keyboard Shortcuts")
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .title_bar(false)
        .frame(
            egui::Frame::NONE
                .fill(Color32::from_rgb(22, 28, 34))
                .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(40)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(28, 22)),
        )
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Keyboard Shortcuts")
                        .size(26.0)
                        .strong()
                        .color(heading),
                );
            });
            ui.add_space(16.0);

            for (title, rows) in sections {
                ui.label(RichText::new(*title).size(17.0).strong().color(heading));
                ui.add_space(4.0);
                Grid::new(*title)
                    .num_columns(2)
                    .spacing([24.0, 6.0])
                    .show(ui, |ui| {
                        for (key, desc) in *rows {
                            ui.label(
                                RichText::new(*key)
                                    .font(FontId::monospace(15.0))
                                    .color(key_color),
                            );
                            ui.label(RichText::new(*desc).size(15.0).color(desc_color));
                            ui.end_row();
                        }
                    });
                ui.add_space(14.0);
            }

            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("H / Esc to close")
                        .font(FontId::monospace(13.0))
                        .color(Color32::from_white_alpha(150)),
                );
            });
        });
}

fn map_uv_to_world_position(u: f32, v: f32) -> (f32, f32) {
    use crate::world_core::chunk::{CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS};

    let world_size_m = WORLD_SIZE_CHUNKS as f32 * CHUNK_SIZE_METERS;
    let u = u.clamp(0.0, 1.0 - f32::EPSILON);
    let v = v.clamp(0.0, 1.0 - f32::EPSILON);
    ((1.0 - v) * world_size_m, u * world_size_m)
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

/// Max gap between two presses of the same favorite key to count as a double-tap
/// (save). A single tap (recall) is delayed by this long before it fires.
const FAVORITE_DOUBLE_TAP_SECONDS: f32 = 0.35;

/// How long the "Saved position N" / "Position N" confirmation text lingers.
const FAVORITE_TOAST_SECONDS: f32 = 1.6;

/// Sim/wall-clock seconds between periodic autosaves while playing, so a crash
/// or power loss between explicit saves loses at most this much progress. The
/// write is cheap (camera metadata + plant spread delta only), so a tight
/// cadence is affordable.
#[cfg(not(target_arch = "wasm32"))]
const AUTOSAVE_INTERVAL_SECONDS: f32 = 30.0;

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

/// Yield control back to the browser for one turn so it can paint a frame (and
/// run the winit redraw) before the caller resumes. Uses `setTimeout(0)` — a
/// macrotask boundary — because a microtask yield (an already-resolved promise)
/// would not let the browser render between steps.
#[cfg(target_arch = "wasm32")]
async fn yield_to_browser() {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        // Schedule a macrotask so the browser can paint before we resume. If the
        // timer can't be scheduled (no window, or set_timeout errors), resolve
        // immediately — a microtask yield is worse for rendering but must not hang
        // the cooperative loader by leaving the promise forever pending.
        let scheduled = web_sys::window().and_then(|win| {
            win.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0)
                .ok()
        });
        if scheduled.is_none() {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}
