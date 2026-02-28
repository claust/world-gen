// Binary entry point — native only.
// On wasm32, the cdylib entry point in lib.rs is used instead.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use winit::dpi::PhysicalSize;
    use winit::event_loop::EventLoop;
    use winit::window::WindowBuilder;

    use world_gen::app::{self, AppState};
    use world_gen::debug_api::DebugApiConfig;

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

    let cursor_captured = app::try_grab_window_cursor(window);
    let app = pollster::block_on(AppState::new(window, debug_api, cursor_captured))?;

    app::run_event_loop(app, event_loop)
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // Wasm entry point is #[wasm_bindgen(start)] in lib.rs.
    // This binary target is not used for wasm builds.
}
