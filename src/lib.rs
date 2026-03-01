#[cfg(not(target_arch = "wasm32"))]
pub mod debug_api;
pub mod renderer_wgpu;
pub mod ui;
pub mod world_core;
pub mod world_runtime;

pub mod app;

#[cfg(target_arch = "wasm32")]
mod web_entry {
    use wasm_bindgen::prelude::*;
    use winit::window::WindowBuilder;

    use crate::app;

    #[wasm_bindgen(start)]
    pub fn wasm_main() {
        console_error_panic_hook::set_once();
        console_log::init_with_level(log::Level::Info).expect("failed to init logger");

        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("world-gen-canvas"))
            .expect("canvas element #world-gen-canvas not found");

        use winit::platform::web::WindowBuilderExtWebSys;
        let event_loop = winit::event_loop::EventLoop::new().expect("failed to create event loop");
        let window = Box::leak(Box::new(
            WindowBuilder::new()
                .with_canvas(Some(canvas.unchecked_into()))
                .build(&event_loop)
                .expect("failed to create window"),
        ));

        app::run_event_loop_web(window, event_loop);
    }
}
