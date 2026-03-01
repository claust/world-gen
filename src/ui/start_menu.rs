use egui::{self, Color32, RichText};

pub enum MenuAction {
    NewGame,
    ResumeGame,
    Exit,
}

pub struct StartMenu {
    save_exists: bool,
}

impl StartMenu {
    pub fn new(save_exists: bool) -> Self {
        Self { save_exists }
    }

    /// Draw the start menu. Returns `Some(action)` when a button is clicked.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<MenuAction> {
        let mut action = None;

        egui::CentralPanel::default()
            .frame({
                #[allow(deprecated)]
                egui::Frame::none().fill(Color32::from_black_alpha(120))
            })
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.3);

                    ui.label(RichText::new("World Gen").size(48.0).color(Color32::WHITE));
                    ui.add_space(40.0);

                    let button_size = egui::vec2(200.0, 50.0);

                    if ui
                        .add_sized(
                            button_size,
                            egui::Button::new(RichText::new("Start Game").size(20.0)),
                        )
                        .clicked()
                    {
                        action = Some(MenuAction::NewGame);
                    }

                    ui.add_space(12.0);

                    if self.save_exists {
                        if ui
                            .add_sized(
                                button_size,
                                egui::Button::new(RichText::new("Resume Game").size(20.0)),
                            )
                            .clicked()
                        {
                            action = Some(MenuAction::ResumeGame);
                        }
                    } else {
                        ui.add_enabled_ui(false, |ui| {
                            ui.add_sized(
                                button_size,
                                egui::Button::new(RichText::new("Resume Game").size(20.0)),
                            );
                        });
                    }

                    ui.add_space(12.0);

                    if ui
                        .add_sized(
                            button_size,
                            egui::Button::new(RichText::new("Exit").size(20.0)),
                        )
                        .clicked()
                    {
                        action = Some(MenuAction::Exit);
                    }
                });
            });

        action
    }
}
