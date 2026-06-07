use egui;

use super::theme;
use super::ui_registry::UiRegistry;

pub enum MenuAction {
    NewGame,
    ResumeGame,
    Herbarium,
    OpenSettings,
    OpenPlantEditor(usize),
    NewPlant,
    LeaveHerbarium,
    LeaveEditor,
    DeletePlant,
    #[cfg(not(target_arch = "wasm32"))]
    EditorScreenshot,
    Exit,
}

/// The FloraForge spruce logo, embedded so it ships in the binary (matches the
/// logo used on the web pages / app icon).
const LOGO_PNG: &[u8] = include_bytes!("../../assets/icon/world-gen.iconset/icon_256x256.png");

pub struct StartMenu {
    save_exists: bool,
    /// Lazily decoded + uploaded the first time the menu draws.
    logo: Option<egui::TextureHandle>,
}

impl StartMenu {
    pub fn new(save_exists: bool) -> Self {
        Self {
            save_exists,
            logo: None,
        }
    }

    /// Decode the embedded logo PNG and upload it as an egui texture (once).
    fn logo_texture(&mut self, ctx: &egui::Context) -> &egui::TextureHandle {
        self.logo.get_or_insert_with(|| {
            let image = image::load_from_memory(LOGO_PNG)
                .expect("embedded logo PNG should decode")
                .to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let color_image =
                egui::ColorImage::from_rgba_unmultiplied(size, image.as_flat_samples().as_slice());
            ctx.load_texture("floraforge-logo", color_image, egui::TextureOptions::LINEAR)
        })
    }

    pub fn set_save_exists(&mut self, value: bool) {
        self.save_exists = value;
    }

    /// Draw the start menu. Returns `Some(action)` when a button is clicked.
    pub fn ui(&mut self, ctx: &egui::Context, registry: &mut UiRegistry) -> Option<MenuAction> {
        let mut action = None;

        let logo = self.logo_texture(ctx).clone();

        egui::CentralPanel::default()
            .frame({
                #[allow(deprecated)]
                egui::Frame::none().fill(theme::OVERLAY_BG)
            })
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.18);

                    ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(128.0, 128.0)));
                    ui.add_space(16.0);

                    ui.label(theme::title("FloraForge", 48.0));
                    ui.add_space(40.0);

                    let button_size = egui::vec2(200.0, 50.0);

                    registry.register_button("btn-start-game", "Start Game");
                    if ui
                        .add_sized(button_size, theme::menu_button("Start Game"))
                        .clicked()
                        || registry.consume_click("btn-start-game")
                    {
                        action = Some(MenuAction::NewGame);
                    }

                    ui.add_space(12.0);

                    if self.save_exists {
                        registry.register_button("btn-resume-game", "Resume Game");
                        if ui
                            .add_sized(button_size, theme::menu_button("Resume Game"))
                            .clicked()
                            || registry.consume_click("btn-resume-game")
                        {
                            action = Some(MenuAction::ResumeGame);
                        }
                    } else {
                        ui.add_enabled_ui(false, |ui| {
                            ui.add_sized(button_size, theme::menu_button("Resume Game"));
                        });
                    }

                    ui.add_space(12.0);

                    registry.register_button("btn-herbarium", "Herbarium");
                    if ui
                        .add_sized(button_size, theme::menu_button("Herbarium"))
                        .clicked()
                        || registry.consume_click("btn-herbarium")
                    {
                        action = Some(MenuAction::Herbarium);
                    }

                    ui.add_space(12.0);

                    registry.register_button("btn-settings", "Settings");
                    if ui
                        .add_sized(button_size, theme::menu_button("Settings"))
                        .clicked()
                        || registry.consume_click("btn-settings")
                    {
                        action = Some(MenuAction::OpenSettings);
                    }

                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        ui.add_space(12.0);

                        registry.register_button("btn-exit", "Exit");
                        if ui
                            .add_sized(button_size, theme::menu_button("Exit"))
                            .clicked()
                            || registry.consume_click("btn-exit")
                        {
                            action = Some(MenuAction::Exit);
                        }
                    }
                });
            });

        action
    }
}
