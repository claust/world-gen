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
                // F1 toggles config panel (intercept before anything else)
                if let WindowEvent::KeyboardInput {
                    event: ref key_event,
                    ..
                } = event
                {
                    if key_event.state == ElementState::Pressed
                        && matches!(key_event.physical_key, PhysicalKey::Code(KeyCode::F1))
                    {
                        if !app.is_on_menu() && !app.is_on_herbarium() && !app.is_on_editor() {
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

                // Feed events to egui when on start menu, config panel, or plant editor
                let egui_wants_event = if app.is_on_menu()
                    || app.is_on_herbarium()
                    || app.config_panel.is_visible()
                    || app.is_on_editor()
                {
                    app.egui_bridge.on_window_event(&event)
                } else {
                    false
                };

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
                        if !app.is_on_menu() {
                            if app.is_on_editor() {
                                app.leave_plant_editor();
                            } else if app.is_on_herbarium() {
                                app.leave_herbarium();
                            } else {
                                if app.config_panel.is_visible() {
                                    app.config_panel.toggle();
                                }
                                #[cfg(not(target_arch = "wasm32"))]
                                app.save_and_update();
                                app.release_cursor();
                                app.return_to_menu();
                            }
                        }
                    }
                    // Left/Right arrow keys for plant editor orbit
                    WindowEvent::KeyboardInput { ref event, .. }
                        if app.is_on_editor()
                            && matches!(
                                event.physical_key,
                                PhysicalKey::Code(KeyCode::ArrowLeft)
                                    | PhysicalKey::Code(KeyCode::ArrowRight)
                            ) =>
                    {
                        let pressed = event.state == ElementState::Pressed;
                        if let Some(editor) = &mut app.plant_editor {
                            match event.physical_key {
                                PhysicalKey::Code(KeyCode::ArrowLeft) => {
                                    editor.orbit_left = pressed;
                                }
                                PhysicalKey::Code(KeyCode::ArrowRight) => {
                                    editor.orbit_right = pressed;
                                }
                                _ => {}
                            }
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } if app.is_on_editor() => {
                        if let Some(editor) = &mut app.plant_editor {
                            editor.on_cursor_move(position.x, position.y);
                        }
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    } if app.focused && !app.cursor_captured => {
                        if app.is_on_editor() && !egui_wants_event {
                            if let Some(editor) = &mut app.plant_editor {
                                editor.on_mouse_press();
                            }
                        } else if app.is_on_menu()
                            || app.is_on_herbarium()
                            || app.config_panel.is_visible()
                            || app.is_on_editor()
                        {
                            // Don't capture cursor on menu, herbarium, config panel, or plant editor
                        } else {
                            app.capture_cursor();
                        }
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Released,
                        button: MouseButton::Left,
                        ..
                    } if app.is_on_editor() => {
                        if let Some(editor) = &mut app.plant_editor {
                            editor.on_mouse_release();
                        }
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
                        if let Some(action) = app.pending_menu_action.take() {
                            use crate::ui::MenuAction;
                            match action {
                                MenuAction::NewGame => app.start_game(false),
                                MenuAction::ResumeGame => app.start_game(true),
                                MenuAction::Herbarium => app.enter_herbarium(),
                                MenuAction::OpenPlantEditor(i) => {
                                    app.enter_plant_editor_for_entry(i)
                                }
                                MenuAction::NewPlant => app.enter_plant_editor_new_plant(),
                                MenuAction::LeaveHerbarium => app.leave_herbarium(),
                                MenuAction::LeaveEditor => app.leave_plant_editor(),
                                MenuAction::DeletePlant => app.delete_current_plant(),
                                MenuAction::Exit => {
                                    #[cfg(not(target_arch = "wasm32"))]
                                    target.exit();
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::DeviceEvent { event, .. } => {
                // Block mouse delta when on start menu, config panel, or plant editor
                if app.is_on_menu()
                    || app.is_on_herbarium()
                    || app.config_panel.is_visible()
                    || app.is_on_editor()
                {
                    // skip device events
                } else {
                    app.process_device_event(&event);
                }
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
                    Ok(mut state) => {
                        // Force a resize with the actual window dimensions.
                        // On Chrome, the initial inner_size() during GPU init may return
                        // stale dimensions before CSS layout has settled (canvas is sized
                        // via 100vw/100vh). Without this, egui lays out UI elements with
                        // wrong screen bounds, making buttons invisible until a manual resize.
                        let actual_size = window.inner_size();
                        state.resize(actual_size);
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
                // F1 toggles config panel
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

                // Feed events to egui when on start menu, config panel, or plant editor
                let egui_wants_event = if app.is_on_menu()
                    || app.is_on_herbarium()
                    || app.config_panel.is_visible()
                    || app.is_on_editor()
                {
                    app.egui_bridge.on_window_event(&event)
                } else {
                    false
                };

                if !egui_wants_event {
                    app.process_window_event(&event);
                }

                match event {
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape)) =>
                    {
                        if !app.is_on_menu() {
                            if app.is_on_editor() {
                                app.leave_plant_editor();
                            } else if app.is_on_herbarium() {
                                app.leave_herbarium();
                            } else {
                                if app.config_panel.is_visible() {
                                    app.config_panel.toggle();
                                }
                                app.release_cursor();
                                app.return_to_menu();
                            }
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } if app.is_on_editor() => {
                        if let Some(editor) = &mut app.plant_editor {
                            editor.on_cursor_move(position.x, position.y);
                        }
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                        ..
                    } if app.focused && !app.cursor_captured => {
                        if app.is_on_editor() && !egui_wants_event {
                            if let Some(editor) = &mut app.plant_editor {
                                editor.on_mouse_press();
                            }
                        } else if app.is_on_menu()
                            || app.is_on_herbarium()
                            || app.config_panel.is_visible()
                            || app.is_on_editor()
                        {
                            // Don't capture cursor on menu, herbarium, config panel, or plant editor
                        } else {
                            app.capture_cursor();
                        }
                    }
                    WindowEvent::MouseInput {
                        state: ElementState::Released,
                        button: MouseButton::Left,
                        ..
                    } if app.is_on_editor() => {
                        if let Some(editor) = &mut app.plant_editor {
                            editor.on_mouse_release();
                        }
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

                        // Process menu actions
                        if let Some(action) = app.pending_menu_action.take() {
                            use crate::ui::MenuAction;
                            match action {
                                MenuAction::NewGame => app.start_game(false),
                                MenuAction::ResumeGame => app.start_game(true),
                                MenuAction::Herbarium => app.enter_herbarium(),
                                MenuAction::OpenPlantEditor(i) => {
                                    app.enter_plant_editor_for_entry(i)
                                }
                                MenuAction::NewPlant => app.enter_plant_editor_new_plant(),
                                MenuAction::LeaveHerbarium => app.leave_herbarium(),
                                MenuAction::LeaveEditor => app.leave_plant_editor(),
                                MenuAction::DeletePlant => app.delete_current_plant(),
                                MenuAction::Exit => {} // no-op on WASM
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::DeviceEvent { event, .. } => {
                if app.is_on_menu()
                    || app.is_on_herbarium()
                    || app.config_panel.is_visible()
                    || app.is_on_editor()
                {
                    // skip device events
                } else {
                    app.process_device_event(&event);
                }
            }
            Event::AboutToWait => {
                app.window.request_redraw();
            }
            _ => {}
        }
    });
}
