use egui;

use super::theme;
use super::ui_registry::UiRegistry;
use crate::world_core::config::AudioConfig;

/// Modal settings dialog shown over the start menu.
///
/// For now it only exposes the master audio volume. It edits the live
/// [`AudioConfig`] in place; the caller is responsible for persisting the
/// config to disk when the dialog is closed (see [`SettingsPanel::ui`]).
#[derive(Default)]
pub struct SettingsPanel {
    open: bool,
}

impl SettingsPanel {
    pub fn new() -> Self {
        Self { open: false }
    }

    pub fn open(&mut self) {
        self.open = true;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Draw the dialog when open, editing `audio` in place. Returns `true` on the
    /// frame the dialog is closed so the caller can persist the new settings.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        registry: &mut UiRegistry,
        audio: &mut AudioConfig,
    ) -> bool {
        if !self.open {
            return false;
        }

        // Register elements for debug-API control / introspection.
        registry.register_slider(
            "slider-master-volume",
            "Master Volume",
            audio.master_volume as f64,
            0.0,
            1.0,
        );
        if let Some(v) = registry.consume_set_value("slider-master-volume") {
            if let Ok(f) = v.parse::<f32>() {
                audio.master_volume = f.clamp(0.0, 1.0);
            }
        }
        registry.register_button("btn-settings-close", "Close");

        let mut window_open = true;
        let mut closed = false;

        // Near-opaque fill so the menu buttons behind the dialog don't bleed
        // through (theme::PANEL_BG is intentionally translucent).
        let fill = egui::Color32::from_rgb(8, 31, 12);
        egui::Window::new(theme::title("Settings", theme::PANEL_TITLE_SIZE))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .frame(
                egui::Frame::window(ctx.style().as_ref())
                    .fill(fill)
                    .inner_margin(egui::Margin::same(16)),
            )
            .open(&mut window_open)
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.spacing_mut().slider_width = 220.0;
                ui.add_space(8.0);

                ui.label(theme::section_header("Audio"));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Volume");
                    ui.add(
                        egui::Slider::new(&mut audio.master_volume, 0.0..=1.0)
                            .custom_formatter(|n, _| format!("{:.0}%", n * 100.0))
                            .custom_parser(|s| {
                                s.trim()
                                    .trim_end_matches('%')
                                    .parse::<f64>()
                                    .ok()
                                    .map(|v| v / 100.0)
                            }),
                    );
                });

                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    if ui
                        .add_sized(egui::vec2(140.0, 40.0), theme::menu_button("Close"))
                        .clicked()
                        || registry.consume_click("btn-settings-close")
                    {
                        closed = true;
                    }
                });
            });

        // The window's title-bar [x] button toggles `window_open` off.
        if !window_open {
            closed = true;
        }
        if closed {
            self.open = false;
        }
        closed
    }
}
