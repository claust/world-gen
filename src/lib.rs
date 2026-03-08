#[cfg(not(target_arch = "wasm32"))]
pub mod debug_api;
pub mod renderer_wgpu;
pub mod ui;
pub mod world_core;
pub mod world_runtime;

pub mod app;

#[cfg(target_arch = "wasm32")]
mod web_entry {
    use std::cell::Cell;
    use std::sync::Once;

    use wasm_bindgen::prelude::*;
    use winit::window::WindowBuilder;

    use crate::app;

    thread_local! {
        static WEB_APP_STARTED: Cell<bool> = const { Cell::new(false) };
    }

    static LOGGER_INIT: Once = Once::new();

    #[wasm_bindgen]
    pub fn start_web_app() -> Result<(), JsValue> {
        console_error_panic_hook::set_once();
        LOGGER_INIT.call_once(|| {
            console_log::init_with_level(log::Level::Info).expect("failed to init logger");
        });

        if WEB_APP_STARTED.with(|started| started.get()) {
            return Ok(());
        }

        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("world-gen-canvas"))
            .ok_or_else(|| JsValue::from_str("canvas element #world-gen-canvas not found"))?;

        use winit::platform::web::WindowBuilderExtWebSys;
        let event_loop = winit::event_loop::EventLoop::new()
            .map_err(|e| JsValue::from_str(&format!("failed to create event loop: {e}")))?;
        let window = Box::leak(Box::new(
            WindowBuilder::new()
                .with_canvas(Some(canvas.unchecked_into()))
                .build(&event_loop)
                .map_err(|e| JsValue::from_str(&format!("failed to create window: {e}")))?,
        ));

        WEB_APP_STARTED.with(|started| started.set(true));
        app::run_event_loop_web(window, event_loop);
        Ok(())
    }
}
