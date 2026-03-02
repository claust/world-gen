use serde::Serialize;

const CROWN_SHAPES: &[&str] = &[
    "conical", "columnar", "dome", "oval", "vase", "umbrella", "weeping", "fan_top",
];
const LENGTH_PROFILES: &[&str] = &["conical", "dome", "columnar", "vase", "layered"];
const FOLIAGE_STYLES: &[&str] = &["broadleaf", "needle", "scale_leaf", "palm_frond", "none"];

#[derive(Clone, Serialize, PartialEq)]
pub struct PlantParams {
    pub crown_shape: String,
    pub length_profile: String,
    pub foliage_style: String,
    pub apical_dominance: f32,
    pub gravity_response: f32,
    pub crown_base: f32,
    pub crown_density: f32,
    pub aspect_ratio: f32,
}

impl Default for PlantParams {
    fn default() -> Self {
        Self {
            crown_shape: "dome".to_string(),
            length_profile: "dome".to_string(),
            foliage_style: "broadleaf".to_string(),
            apical_dominance: 0.3,
            gravity_response: 0.45,
            crown_base: 0.25,
            crown_density: 0.7,
            aspect_ratio: 1.3,
        }
    }
}

pub struct PlantEditorPanel {
    params: PlantParams,
    last_applied: PlantParams,
    dirty: bool,
}

impl Default for PlantEditorPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl PlantEditorPanel {
    pub fn new() -> Self {
        Self {
            params: PlantParams::default(),
            last_applied: PlantParams::default(),
            dirty: false,
        }
    }

    /// Returns changed params when the pointer is released after changes.
    /// Debounces so the tree doesn't regenerate mid-drag.
    pub fn take_dirty_params(&mut self, ctx: &egui::Context) -> Option<PlantParams> {
        if !self.dirty {
            return None;
        }
        let released = ctx.input(|i| i.pointer.any_released());
        if released {
            self.dirty = false;
            self.last_applied = self.params.clone();
            Some(self.params.clone())
        } else {
            None
        }
    }

    /// Draw the plant editor side panel. Returns `true` if "Back to Menu" was clicked.
    pub fn ui(&mut self, ctx: &egui::Context) -> bool {
        let mut back = false;

        egui::SidePanel::left("plant_editor_panel")
            .default_width(320.0)
            .frame(
                egui::Frame::side_top_panel(ctx.style().as_ref())
                    .fill(egui::Color32::from_rgba_unmultiplied(30, 30, 30, 220)),
            )
            .show(ctx, |ui| {
                ui.heading("Plant Editor");
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::ComboBox::from_label("Crown Shape")
                        .selected_text(&self.params.crown_shape)
                        .show_ui(ui, |ui| {
                            for shape in CROWN_SHAPES {
                                ui.selectable_value(
                                    &mut self.params.crown_shape,
                                    shape.to_string(),
                                    *shape,
                                );
                            }
                        });

                    egui::ComboBox::from_label("Length Profile")
                        .selected_text(&self.params.length_profile)
                        .show_ui(ui, |ui| {
                            for profile in LENGTH_PROFILES {
                                ui.selectable_value(
                                    &mut self.params.length_profile,
                                    profile.to_string(),
                                    *profile,
                                );
                            }
                        });

                    egui::ComboBox::from_label("Foliage Style")
                        .selected_text(&self.params.foliage_style)
                        .show_ui(ui, |ui| {
                            for style in FOLIAGE_STYLES {
                                ui.selectable_value(
                                    &mut self.params.foliage_style,
                                    style.to_string(),
                                    *style,
                                );
                            }
                        });

                    ui.add_space(8.0);

                    ui.add(
                        egui::Slider::new(&mut self.params.apical_dominance, 0.0..=1.0)
                            .text("Apical Dominance"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.params.gravity_response, 0.0..=1.0)
                            .text("Gravity Response"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.params.crown_base, 0.0..=0.8)
                            .text("Crown Base"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.params.crown_density, 0.2..=1.0)
                            .text("Crown Density"),
                    );
                    ui.add(
                        egui::Slider::new(&mut self.params.aspect_ratio, 0.5..=2.0)
                            .text("Aspect Ratio"),
                    );

                    ui.add_space(8.0);
                    ui.separator();

                    if ui.button("Reset Defaults").clicked() {
                        self.params = PlantParams::default();
                        self.dirty = true;
                    }

                    ui.add_space(12.0);

                    if ui.button("Back to Menu").clicked() {
                        back = true;
                    }
                });
            });

        if self.params != self.last_applied {
            self.dirty = true;
        }

        back
    }
}
