use anyhow::Result;
use wgpu::SurfaceError;
use winit::event::{ElementState, Event, MouseButton, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};

use super::AppState;

pub fn run_event_loop(mut app: AppState, event_loop: EventLoop<()>) -> Result<()> {
    event_loop.run(move |event, target| {
        target.set_control_flow(ControlFlow::Poll);

        match event {
            Event::WindowEvent { window_id, event } if window_id == app.window.id() => {
                // F1 toggles config panel (intercept before anything else) [native only]
                #[cfg(not(target_arch = "wasm32"))]
                if let WindowEvent::KeyboardInput {
                    event: ref key_event,
                    ..
                } = event
                {
                    if key_event.state == ElementState::Pressed
                        && matches!(key_event.physical_key, PhysicalKey::Code(KeyCode::F1))
                    {
                        if !app.is_on_menu() {
                            app.config_panel.toggle();
                            if app.config_panel.is_visible() {
                                app.release_cursor();
                            } else {
                                app.capture_cursor();
                            }
                        }
                        return;
                    }
                }

                // Feed events to egui when on start menu or config panel visible [native only]
                #[cfg(not(target_arch = "wasm32"))]
                let egui_wants_event = if app.is_on_menu() || app.config_panel.is_visible() {
                    app.egui_bridge.on_window_event(&event)
                } else {
                    false
                };
                #[cfg(target_arch = "wasm32")]
                let egui_wants_event = false;

                // Only forward to camera if egui didn't consume the event
                if !egui_wants_event {
                    app.process_window_event(&event);
                }

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
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            if !app.is_on_menu() {
                                if app.config_panel.is_visible() {
                                    app.config_panel.toggle();
                                    app.capture_cursor();
                                } else {
                                    app.release_cursor();
                                }
                            }
                        }
                        #[cfg(target_arch = "wasm32")]
                        app.release_cursor();
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    } if app.focused && !app.cursor_captured => {
                        #[cfg(not(target_arch = "wasm32"))]
                        if app.is_on_menu() || app.config_panel.is_visible() {
                            // Don't capture cursor on start menu or when config panel is open
                        } else {
                            app.capture_cursor();
                        }
                        #[cfg(target_arch = "wasm32")]
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
                            Err(e) => {
                                log::error!("surface error: {e}");
                            }
                        }

                        // Process menu actions (needs access to `target` for Exit)
                        #[cfg(not(target_arch = "wasm32"))]
                        if let Some(action) = app.pending_menu_action.take() {
                            use crate::ui::MenuAction;
                            match action {
                                MenuAction::NewGame => app.start_game(false),
                                MenuAction::ResumeGame => app.start_game(true),
                                MenuAction::Exit => target.exit(),
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::DeviceEvent { event, .. } => {
                // Block mouse delta when on start menu or config panel is visible [native only]
                #[cfg(not(target_arch = "wasm32"))]
                if app.is_on_menu() || app.config_panel.is_visible() {
                    // skip device events
                } else {
                    app.process_device_event(&event);
                }
                #[cfg(target_arch = "wasm32")]
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
pub fn run_event_loop_web(window: &'static winit::window::Window, event_loop: EventLoop<()>) {
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
                            Err(e) => {
                                log::error!("surface error: {e}");
                            }
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
