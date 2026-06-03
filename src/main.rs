// Binary entry point — native only.
// On wasm32, the cdylib entry point in lib.rs is used instead.
#![allow(unexpected_cfgs)] // objc crate's msg_send! macro checks cfg(feature = "cargo-clippy")

#[cfg(target_os = "macos")]
fn set_macos_dock_icon() {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    let icon_data = include_bytes!("../assets/icon/icon_1024.png");
    unsafe {
        let ns_data: *mut Object = msg_send![Class::get("NSData").unwrap(), dataWithBytes:icon_data.as_ptr() length:icon_data.len()];
        let alloc: *mut Object = msg_send![Class::get("NSImage").unwrap(), alloc];
        let ns_image: *mut Object = msg_send![alloc, initWithData: ns_data];
        let app: *mut Object = msg_send![Class::get("NSApplication").unwrap(), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: ns_image];
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> anyhow::Result<()> {
    use anyhow::Context;
    use winit::dpi::PhysicalSize;
    use winit::event_loop::EventLoop;
    use winit::window::WindowBuilder;

    use world_gen::app::{self, AppState};
    use world_gen::debug_api::DebugApiConfig;

    // Default to info for our crate (so world-generation progress and other
    // status logs show) while keeping the chatty GPU/windowing deps at warn.
    // RUST_LOG, if set, overrides this entirely.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,world_gen=info"),
    )
    .init();
    let debug_api = DebugApiConfig::from_env_args()?;
    log::info!(
        "debug api enabled: {}, bind: {}",
        debug_api.enabled,
        debug_api.bind_addr
    );

    let benchmark_path = parse_benchmark_arg();

    let event_loop = EventLoop::new()?;

    #[cfg(target_os = "macos")]
    set_macos_dock_icon();

    let window = Box::leak(Box::new(
        WindowBuilder::new()
            .with_title("FloraForge")
            .with_inner_size(PhysicalSize::new(1600, 900))
            .build(&event_loop)
            .context("failed to create window")?,
    ));

    // Don't capture cursor at startup — start screen needs a free cursor
    let app = pollster::block_on(AppState::new(window, debug_api, false, benchmark_path))?;

    app::run_event_loop(app, event_loop)
}

/// Parses `--benchmark [path]`. The path is optional and defaults to
/// `benchmarks/flythrough.json`. Returns `None` when the flag is absent.
#[cfg(not(target_arch = "wasm32"))]
fn parse_benchmark_arg() -> Option<std::path::PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--benchmark" {
            let path = args
                .next()
                .filter(|s| !s.starts_with("--"))
                .unwrap_or_else(|| "benchmarks/flythrough.json".to_string());
            return Some(std::path::PathBuf::from(path));
        }
    }
    None
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // Wasm entry point is #[wasm_bindgen(start)] in lib.rs.
    // This binary target is not used for wasm builds.
}
