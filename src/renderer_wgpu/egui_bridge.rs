use egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorIcon, Window};

pub struct EguiBridge {
    ctx: Context,
    events: Vec<Event>,
    pointer_pos: Pos2,
    modifiers: Modifiers,
    pixels_per_point: f32,
    screen_size: (u32, u32),
}

impl EguiBridge {
    pub fn new(pixels_per_point: f32, width: u32, height: u32) -> Self {
        Self {
            ctx: Context::default(),
            events: Vec::new(),
            pointer_pos: Pos2::ZERO,
            modifiers: Modifiers::NONE,
            pixels_per_point,
            screen_size: (width, height),
        }
    }

    pub fn ctx(&self) -> &Context {
        &self.ctx
    }

    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.screen_size = (width, height);
    }

    /// Feed a winit 0.29 WindowEvent. Returns true if egui wants this event
    /// (pointer is over an egui area, or a text field is focused).
    pub fn on_window_event(&mut self, event: &WindowEvent) -> bool {
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                let pos = Pos2::new(
                    position.x as f32 / self.pixels_per_point,
                    position.y as f32 / self.pixels_per_point,
                );
                self.pointer_pos = pos;
                self.events.push(Event::PointerMoved(pos));
                self.ctx.wants_pointer_input()
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(egui_button) = winit_button_to_egui(*button) {
                    self.events.push(Event::PointerButton {
                        pos: self.pointer_pos,
                        button: egui_button,
                        pressed: *state == ElementState::Pressed,
                        modifiers: self.modifiers,
                    });
                }
                self.ctx.wants_pointer_input()
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let scroll = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Vec2::new(*x, *y) * 24.0,
                    MouseScrollDelta::PixelDelta(d) => {
                        Vec2::new(d.x as f32, d.y as f32) / self.pixels_per_point
                    }
                };
                self.events.push(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: scroll,
                    modifiers: self.modifiers,
                });
                self.ctx.wants_pointer_input()
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;

                // Update modifier tracking
                if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        KeyCode::ShiftLeft | KeyCode::ShiftRight => {
                            self.modifiers.shift = pressed;
                        }
                        KeyCode::ControlLeft | KeyCode::ControlRight => {
                            self.modifiers.ctrl = pressed;
                            #[cfg(not(target_os = "macos"))]
                            {
                                self.modifiers.command = pressed;
                            }
                        }
                        KeyCode::AltLeft | KeyCode::AltRight => {
                            self.modifiers.alt = pressed;
                        }
                        KeyCode::SuperLeft | KeyCode::SuperRight => {
                            self.modifiers.mac_cmd = pressed;
                            #[cfg(target_os = "macos")]
                            {
                                self.modifiers.command = pressed;
                            }
                        }
                        _ => {}
                    }
                }

                // Key event
                if let PhysicalKey::Code(code) = event.physical_key {
                    if let Some(key) = winit_key_to_egui(code) {
                        self.events.push(Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers: self.modifiers,
                        });
                    }
                }

                // Text input (only on press, skip control characters)
                if pressed {
                    if let Some(ref text) = event.text {
                        let text_str: &str = text;
                        for ch in text_str.chars() {
                            if !ch.is_control() {
                                self.events.push(Event::Text(ch.to_string()));
                            }
                        }
                    }
                }

                self.ctx.wants_keyboard_input()
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.pixels_per_point = *scale_factor as f32;
                false
            }

            _ => false,
        }
    }

    /// Drain accumulated events into a RawInput for this frame.
    pub fn take_raw_input(&mut self) -> RawInput {
        let (w, h) = self.screen_size;
        let screen_rect = Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(
                w as f32 / self.pixels_per_point,
                h as f32 / self.pixels_per_point,
            ),
        );

        let mut raw = RawInput {
            screen_rect: Some(screen_rect),
            events: std::mem::take(&mut self.events),
            modifiers: self.modifiers,
            ..Default::default()
        };
        // In egui 0.30, pixels_per_point is set per-viewport
        raw.viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(self.pixels_per_point);
        raw
    }

    /// Apply egui platform output (cursor icon changes).
    pub fn handle_platform_output(&self, window: &Window, output: &egui::PlatformOutput) {
        let cursor = match output.cursor_icon {
            egui::CursorIcon::Default => CursorIcon::Default,
            egui::CursorIcon::PointingHand => CursorIcon::Pointer,
            egui::CursorIcon::Text => CursorIcon::Text,
            egui::CursorIcon::Crosshair => CursorIcon::Crosshair,
            egui::CursorIcon::Grab => CursorIcon::Grab,
            egui::CursorIcon::Grabbing => CursorIcon::Grabbing,
            egui::CursorIcon::ResizeHorizontal => CursorIcon::EwResize,
            egui::CursorIcon::ResizeVertical => CursorIcon::NsResize,
            _ => CursorIcon::Default,
        };
        window.set_cursor_icon(cursor);
    }
}

fn winit_button_to_egui(button: MouseButton) -> Option<egui::PointerButton> {
    match button {
        MouseButton::Left => Some(egui::PointerButton::Primary),
        MouseButton::Right => Some(egui::PointerButton::Secondary),
        MouseButton::Middle => Some(egui::PointerButton::Middle),
        _ => None,
    }
}

fn winit_key_to_egui(code: KeyCode) -> Option<Key> {
    match code {
        KeyCode::ArrowDown => Some(Key::ArrowDown),
        KeyCode::ArrowUp => Some(Key::ArrowUp),
        KeyCode::ArrowLeft => Some(Key::ArrowLeft),
        KeyCode::ArrowRight => Some(Key::ArrowRight),
        KeyCode::Escape => Some(Key::Escape),
        KeyCode::Tab => Some(Key::Tab),
        KeyCode::Backspace => Some(Key::Backspace),
        KeyCode::Enter | KeyCode::NumpadEnter => Some(Key::Enter),
        KeyCode::Space => Some(Key::Space),
        KeyCode::Delete => Some(Key::Delete),
        KeyCode::Home => Some(Key::Home),
        KeyCode::End => Some(Key::End),
        KeyCode::PageUp => Some(Key::PageUp),
        KeyCode::PageDown => Some(Key::PageDown),
        KeyCode::KeyA => Some(Key::A),
        KeyCode::KeyB => Some(Key::B),
        KeyCode::KeyC => Some(Key::C),
        KeyCode::KeyD => Some(Key::D),
        KeyCode::KeyE => Some(Key::E),
        KeyCode::KeyF => Some(Key::F),
        KeyCode::KeyG => Some(Key::G),
        KeyCode::KeyH => Some(Key::H),
        KeyCode::KeyI => Some(Key::I),
        KeyCode::KeyJ => Some(Key::J),
        KeyCode::KeyK => Some(Key::K),
        KeyCode::KeyL => Some(Key::L),
        KeyCode::KeyM => Some(Key::M),
        KeyCode::KeyN => Some(Key::N),
        KeyCode::KeyO => Some(Key::O),
        KeyCode::KeyP => Some(Key::P),
        KeyCode::KeyQ => Some(Key::Q),
        KeyCode::KeyR => Some(Key::R),
        KeyCode::KeyS => Some(Key::S),
        KeyCode::KeyT => Some(Key::T),
        KeyCode::KeyU => Some(Key::U),
        KeyCode::KeyV => Some(Key::V),
        KeyCode::KeyW => Some(Key::W),
        KeyCode::KeyX => Some(Key::X),
        KeyCode::KeyY => Some(Key::Y),
        KeyCode::KeyZ => Some(Key::Z),
        KeyCode::Digit0 | KeyCode::Numpad0 => Some(Key::Num0),
        KeyCode::Digit1 | KeyCode::Numpad1 => Some(Key::Num1),
        KeyCode::Digit2 | KeyCode::Numpad2 => Some(Key::Num2),
        KeyCode::Digit3 | KeyCode::Numpad3 => Some(Key::Num3),
        KeyCode::Digit4 | KeyCode::Numpad4 => Some(Key::Num4),
        KeyCode::Digit5 | KeyCode::Numpad5 => Some(Key::Num5),
        KeyCode::Digit6 | KeyCode::Numpad6 => Some(Key::Num6),
        KeyCode::Digit7 | KeyCode::Numpad7 => Some(Key::Num7),
        KeyCode::Digit8 | KeyCode::Numpad8 => Some(Key::Num8),
        KeyCode::Digit9 | KeyCode::Numpad9 => Some(Key::Num9),
        _ => None,
    }
}
