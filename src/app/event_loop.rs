use anyhow::Result;
use wgpu::SurfaceError;
use winit::event::{ElementState, Event, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};

use super::AppState;

/// Normalize a mouse-wheel delta to "scroll lines" (one notch ≈ 1.0). Line and
/// pixel deltas (trackpads report the latter) are mapped onto the same scale so
/// editor zoom feels consistent across input devices.
fn scroll_lines(delta: MouseScrollDelta) -> f32 {
    match delta {
        MouseScrollDelta::LineDelta(_, y) => y,
        MouseScrollDelta::PixelDelta(pos) => pos.y as f32 / 50.0,
    }
}

// Set the macOS dock icon to the app icon. Must run *after* the app has
// finished launching — winit only promotes an un-bundled binary to a regular
// dock app (activation policy + dock tile) once the event loop is running, so
// calling this before then silently leaves the generic "exec" icon. We invoke
// it once on the first redraw.
#[cfg(target_os = "macos")]
fn set_macos_dock_icon() {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    let icon_data = include_bytes!("../../assets/icon/icon_1024.png");
    unsafe {
        let ns_data: *mut Object = msg_send![Class::get("NSData").unwrap(), dataWithBytes:icon_data.as_ptr() length:icon_data.len()];
        let alloc: *mut Object = msg_send![Class::get("NSImage").unwrap(), alloc];
        let ns_image: *mut Object = msg_send![alloc, initWithData: ns_data];
        let app: *mut Object = msg_send![Class::get("NSApplication").unwrap(), sharedApplication];
        let _: () = msg_send![app, setApplicationIconImage: ns_image];
    }
}

pub fn run_event_loop(mut app: AppState, event_loop: EventLoop<()>) -> Result<()> {
    #[cfg(target_os = "macos")]
    let mut dock_icon_set = false;

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
                        if !app.is_on_menu()
                            && !app.is_loading()
                            && !app.is_on_herbarium()
                            && !app.is_on_editor()
                        {
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

                // M toggles the full-world map overlay (intercept before camera)
                if let WindowEvent::KeyboardInput {
                    event: ref key_event,
                    ..
                } = event
                {
                    if key_event.state == ElementState::Pressed
                        && matches!(key_event.physical_key, PhysicalKey::Code(KeyCode::KeyM))
                    {
                        if !app.is_on_menu()
                            && !app.is_loading()
                            && !app.is_on_herbarium()
                            && !app.is_on_editor()
                            && !app.config_panel.is_visible()
                        {
                            app.toggle_map_overlay();
                        }
                        return;
                    }
                }

                // P captures a screenshot and copies it to the system clipboard
                #[cfg(not(target_arch = "wasm32"))]
                if let WindowEvent::KeyboardInput {
                    event: ref key_event,
                    ..
                } = event
                {
                    // Guarded like `M`: `P` is a printable character, so don't
                    // swallow it from egui text fields (menu seed input, config
                    // panel, editor, herbarium).
                    if key_event.state == ElementState::Pressed
                        && matches!(key_event.physical_key, PhysicalKey::Code(KeyCode::KeyP))
                        && !app.is_on_menu()
                        && !app.is_loading()
                        && !app.is_on_herbarium()
                        && !app.is_on_editor()
                        && !app.config_panel.is_visible()
                        && app.screenshot_pending.is_none()
                    {
                        app.screenshot_pending = Some("clipboard-screenshot".to_string());
                        app.screenshot_to_clipboard = true;
                        return;
                    }
                }

                // Feed events to egui when on start menu, config panel, plant
                // editor, or the map overlay
                let egui_wants_event = if app.is_on_menu()
                    || app.is_loading()
                    || app.is_on_herbarium()
                    || app.config_panel.is_visible()
                    || app.is_on_editor()
                    || app.map_open
                {
                    app.egui_bridge.on_window_event(&event)
                } else {
                    false
                };

                // Forward to camera only if egui didn't consume it and the map
                // overlay isn't open (the map freezes camera movement).
                if !egui_wants_event && !app.map_open {
                    app.process_window_event(&event);
                }

                match event {
                    WindowEvent::CloseRequested => {
                        app.save_game();
                        target.exit();
                    }
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape)) =>
                    {
                        if app.map_open {
                            app.toggle_map_overlay();
                        } else if !app.is_on_menu() && !app.is_loading() {
                            if app.is_on_editor() {
                                app.leave_plant_editor();
                            } else if app.is_on_herbarium() {
                                app.leave_herbarium();
                            } else {
                                if app.config_panel.is_visible() {
                                    app.config_panel.toggle();
                                }
                                let _ = app.save_and_update();
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
                    WindowEvent::MouseWheel { delta, .. }
                        if app.is_on_editor() && !egui_wants_event =>
                    {
                        if let Some(editor) = &mut app.plant_editor {
                            editor.on_scroll(scroll_lines(delta));
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
                            || app.is_loading()
                            || app.is_on_herbarium()
                            || app.config_panel.is_visible()
                            || app.is_on_editor()
                            || app.map_open
                        {
                            // Don't capture cursor on menu, loading, herbarium, config panel, plant editor, or map overlay
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
                        // First redraw: the app is now a launched, active dock
                        // app, so the icon override sticks (see fn comment).
                        #[cfg(target_os = "macos")]
                        if !dock_icon_set {
                            set_macos_dock_icon();
                            dock_icon_set = true;
                        }

                        #[cfg(not(target_arch = "wasm32"))]
                        let t_update = std::time::Instant::now();
                        app.update();
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            app.update_cpu_ms = t_update.elapsed().as_secs_f32() * 1000.0;
                        }
                        match app.render() {
                            Ok(()) => {}
                            Err(SurfaceError::Lost) => app.resize(app.gpu.size),
                            Err(SurfaceError::OutOfMemory) => target.exit(),
                            Err(SurfaceError::Timeout | SurfaceError::Outdated) => {}
                            Err(e) => {
                                log::error!("surface error: {e}");
                            }
                        }

                        // Benchmark run complete: report written, exit the app.
                        #[cfg(not(target_arch = "wasm32"))]
                        if app.benchmark_finished() {
                            target.exit();
                        }

                        // Process menu actions (needs access to `target` for Exit)
                        if let Some(action) = app.pending_menu_action.take() {
                            use crate::ui::MenuAction;
                            match action {
                                MenuAction::NewGame => app.begin_loading(false),
                                MenuAction::ResumeGame => app.begin_loading(true),
                                MenuAction::Herbarium => app.enter_herbarium(),
                                MenuAction::OpenSettings => app.settings_panel.open(),
                                MenuAction::OpenPlantEditor(i) => {
                                    app.enter_plant_editor_for_entry(i)
                                }
                                MenuAction::NewPlant => app.enter_plant_editor_new_plant(),
                                MenuAction::LeaveHerbarium => app.leave_herbarium(),
                                MenuAction::LeaveEditor => app.leave_plant_editor(),
                                MenuAction::DeletePlant => app.delete_current_plant(),
                                #[cfg(not(target_arch = "wasm32"))]
                                MenuAction::EditorScreenshot => {
                                    app.screenshot_pending = Some("editor-screenshot".to_string());
                                }
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
                // Block mouse delta when on start menu, loading, config panel, or plant editor
                if app.is_on_menu()
                    || app.is_loading()
                    || app.is_on_herbarium()
                    || app.config_panel.is_visible()
                    || app.is_on_editor()
                    || app.map_open
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

#[cfg(test)]
mod tests {
    use super::*;
    use winit::dpi::PhysicalPosition;

    #[test]
    fn scroll_lines_maps_both_delta_kinds() {
        // Line deltas pass through; positive y (scroll up) stays positive.
        assert_eq!(scroll_lines(MouseScrollDelta::LineDelta(0.0, 1.0)), 1.0);
        assert_eq!(scroll_lines(MouseScrollDelta::LineDelta(0.0, -3.0)), -3.0);
        // Pixel deltas (trackpads) are scaled down and keep their sign.
        assert_eq!(
            scroll_lines(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                0.0, 100.0
            ))),
            2.0
        );
        assert!(
            scroll_lines(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                0.0, -50.0
            ))) < 0.0
        );
    }
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
                        if !app.is_on_menu() && !app.is_loading() {
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

                // M toggles the full-world map overlay (intercept before camera)
                if let WindowEvent::KeyboardInput {
                    event: ref key_event,
                    ..
                } = event
                {
                    if key_event.state == ElementState::Pressed
                        && matches!(key_event.physical_key, PhysicalKey::Code(KeyCode::KeyM))
                    {
                        if !app.is_on_menu()
                            && !app.is_loading()
                            && !app.is_on_herbarium()
                            && !app.is_on_editor()
                            && !app.config_panel.is_visible()
                        {
                            app.toggle_map_overlay();
                        }
                        return;
                    }
                }

                // Feed events to egui when on start menu, loading, config panel,
                // plant editor, or the map overlay
                let egui_wants_event = if app.is_on_menu()
                    || app.is_loading()
                    || app.is_on_herbarium()
                    || app.config_panel.is_visible()
                    || app.is_on_editor()
                    || app.map_open
                {
                    app.egui_bridge.on_window_event(&event)
                } else {
                    false
                };

                if !egui_wants_event && !app.map_open {
                    app.process_window_event(&event);
                }

                match event {
                    WindowEvent::KeyboardInput { event, .. }
                        if event.state == ElementState::Pressed
                            && matches!(event.physical_key, PhysicalKey::Code(KeyCode::Escape)) =>
                    {
                        if app.map_open {
                            app.toggle_map_overlay();
                        } else if !app.is_on_menu() && !app.is_loading() {
                            if app.is_on_editor() {
                                app.leave_plant_editor();
                            } else if app.is_on_herbarium() {
                                app.leave_herbarium();
                            } else {
                                if app.config_panel.is_visible() {
                                    app.config_panel.toggle();
                                }
                                let _ = app.save_and_update();
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
                    WindowEvent::MouseWheel { delta, .. }
                        if app.is_on_editor() && !egui_wants_event =>
                    {
                        if let Some(editor) = &mut app.plant_editor {
                            editor.on_scroll(scroll_lines(delta));
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
                            || app.is_loading()
                            || app.is_on_herbarium()
                            || app.config_panel.is_visible()
                            || app.is_on_editor()
                            || app.map_open
                        {
                            // Don't capture cursor on menu, loading, herbarium, config panel, plant editor, or map overlay
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
                                MenuAction::NewGame => app.begin_loading(false),
                                MenuAction::ResumeGame => app.begin_loading(true),
                                MenuAction::Herbarium => app.enter_herbarium(),
                                MenuAction::OpenSettings => app.settings_panel.open(),
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
                    || app.is_loading()
                    || app.is_on_herbarium()
                    || app.config_panel.is_visible()
                    || app.is_on_editor()
                    || app.map_open
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
