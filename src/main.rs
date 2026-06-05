// Binary entry point — native only.
// On wasm32, the cdylib entry point in lib.rs is used instead.
// In release builds on Windows, use the "windows" subsystem so launching the
// app doesn't also pop up a background console window. Debug builds keep the
// console subsystem so `cargo run` still shows env_logger output in a terminal.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
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
    // Headless world-base generation: produce `world_base.bin` and exit, without
    // creating a window or GPU context. The release workflow runs this on a CI
    // runner to build the snapshot it uploads as a release asset (the same bytes
    // a first-time New Game would otherwise compute locally).
    if let Some(out_path) = parse_generate_base_arg() {
        return generate_base_world(&out_path);
    }

    let debug_api = DebugApiConfig::from_env_args()?;
    log::info!(
        "debug api enabled: {}, bind: {}",
        debug_api.enabled,
        debug_api.bind_addr
    );

    let benchmark_path = parse_benchmark_arg();

    let instance = parse_instance_arg()?;
    if let Some(name) = &instance {
        world_gen::world_core::storage::validate_instance_name(name)?;
        log::info!("instance name: {name}");
    }
    let window_title = match &instance {
        Some(name) => format!("[{name}] FloraForge"),
        None => "FloraForge".to_string(),
    };

    let event_loop = EventLoop::new()?;

    #[cfg(target_os = "macos")]
    set_macos_dock_icon();

    let window = Box::leak(Box::new(
        WindowBuilder::new()
            .with_title(window_title)
            .with_inner_size(PhysicalSize::new(1600, 900))
            .build(&event_loop)
            .context("failed to create window")?,
    ));

    // Don't capture cursor at startup — start screen needs a free cursor
    let app = pollster::block_on(AppState::new(
        window,
        debug_api,
        false,
        benchmark_path,
        instance,
    ))?;

    app::run_event_loop(app, event_loop)
}

/// Parses `--instance-name <name>`, falling back to the `WORLD_GEN_INSTANCE`
/// env var. A named instance namespaces all on-disk state (save, config,
/// plants, captures) under `instances/<name>/` so multiple copies of the game
/// can run on one machine without clobbering each other. Returns `None` for the
/// default single-instance layout.
#[cfg(not(target_arch = "wasm32"))]
fn parse_instance_arg() -> anyhow::Result<Option<String>> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--instance-name" {
            let value = args
                .next()
                .filter(|s| !s.starts_with("--"))
                .ok_or_else(|| anyhow::anyhow!("--instance-name requires a value"))?;
            return Ok(Some(value));
        }
    }
    Ok(std::env::var("WORLD_GEN_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty()))
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

/// Parses `--generate-base [path]`. The path is optional and defaults to
/// `world_base.bin`. Returns `None` when the flag is absent.
#[cfg(not(target_arch = "wasm32"))]
fn parse_generate_base_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--generate-base" {
            let path = args
                .next()
                .filter(|s| !s.starts_with("--"))
                .unwrap_or_else(|| "world_base.bin".to_string());
            return Some(path);
        }
    }
    None
}

/// Generate the canonical base world and write its snapshot to `out_path`. Pure
/// CPU work (no window/GPU), so it runs headless in CI. Uses the same config,
/// herbarium, registry, seed and generation key the game would, so the bytes are
/// byte-for-byte interchangeable with a locally generated cache and pass
/// `PlantWorld::from_base_snapshot` validation on any client built from the same
/// inputs.
#[cfg(not(target_arch = "wasm32"))]
fn generate_base_world(out_path: &str) -> anyhow::Result<()> {
    use std::sync::Arc;
    use world_gen::world_core::config::GameConfig;
    use world_gen::world_core::herbarium::{Herbarium, PlantRegistry};
    use world_gen::world_core::storage::create_storage;
    use world_gen::world_runtime::PlantWorld;

    // Default (unnamed) storage layout: roots at the working directory, matching a
    // fresh first-time New Game — so the generated snapshot's seed and generation
    // key line up with what a shipped client validates the download against.
    let storage = create_storage(None);
    let config = GameConfig::load(&*storage);
    let herbarium = Herbarium::load(&*storage);
    let registry = Arc::new(PlantRegistry::from_herbarium(&herbarium));
    let gen_key = herbarium.generation_key(&config);
    let seed = config.world.seed;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    log::info!("generating base world (seed {seed}, gen_key {gen_key:#018x}, {threads} threads)…");
    let world = PlantWorld::generate_base(seed, &config, registry, threads, None);
    // This is the snapshot shipped on GitHub and downloaded by clients, so use
    // the high download quality to minimize the download (the encode cost is
    // paid here in CI, never on a player's machine).
    let bytes = world.serialize_base(gen_key, PlantWorld::DOWNLOAD_QUALITY);
    std::fs::write(out_path, &bytes)?;
    log::info!(
        "wrote {out_path}: {} plants across {} chunks, {} bytes",
        world.population(),
        world.populated_chunks(),
        bytes.len(),
    );
    Ok(())
}

#[cfg(target_arch = "wasm32")]
fn main() {
    // Wasm entry point is #[wasm_bindgen(start)] in lib.rs.
    // This binary target is not used for wasm builds.
}
