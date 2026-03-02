use rand::Rng;
use serde::Serialize;

use crate::world_core::plant_gen::config::SpeciesConfig;

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

impl PlantParams {
    /// Extract UI-editable parameters from a species config.
    pub fn from_species(spec: &SpeciesConfig) -> Self {
        Self {
            crown_shape: spec.crown.shape.clone(),
            length_profile: spec.branching.length_profile.clone(),
            foliage_style: spec.foliage.style.clone(),
            apical_dominance: spec.branching.apical_dominance,
            gravity_response: spec.branching.gravity_response,
            crown_base: spec.crown.crown_base,
            crown_density: spec.crown.density,
            aspect_ratio: spec.crown.aspect_ratio,
        }
    }

    pub fn randomize() -> Self {
        let mut rng = rand::rng();
        Self {
            crown_shape: CROWN_SHAPES[rng.random_range(0..CROWN_SHAPES.len())].to_string(),
            length_profile: LENGTH_PROFILES[rng.random_range(0..LENGTH_PROFILES.len())].to_string(),
            foliage_style: FOLIAGE_STYLES[rng.random_range(0..FOLIAGE_STYLES.len())].to_string(),
            apical_dominance: rng.random_range(0.0..1.0),
            gravity_response: rng.random_range(0.0..1.0),
            crown_base: rng.random_range(0.0..0.8),
            crown_density: rng.random_range(0.2..1.0),
            aspect_ratio: rng.random_range(0.5..2.0),
        }
    }
}

pub struct PlantEditorPanel {
    params: PlantParams,
    last_applied: PlantParams,
    dirty: bool,
    selected_species: String,
    species_names: Vec<String>,
    species_changed: Option<String>,
}

impl Default for PlantEditorPanel {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

impl PlantEditorPanel {
    pub fn new(species_names: Vec<String>) -> Self {
        let selected = species_names.first().cloned().unwrap_or_default();
        Self {
            params: PlantParams::default(),
            last_applied: PlantParams::default(),
            dirty: false,
            selected_species: selected,
            species_names,
            species_changed: None,
        }
    }

    /// Push new params from outside (e.g. after a species preset change).
    pub fn set_params(&mut self, params: PlantParams) {
        self.params = params.clone();
        self.last_applied = params;
        self.dirty = false;
    }

    /// Take a pending species change, if any.
    pub fn take_species_change(&mut self) -> Option<String> {
        self.species_changed.take()
    }

    pub fn set_species_names(&mut self, names: Vec<String>) {
        if self.selected_species.is_empty() {
            self.selected_species = names.first().cloned().unwrap_or_default();
        }
        self.species_names = names;
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
                    let prev_species = self.selected_species.clone();
                    egui::ComboBox::from_label("Species")
                        .selected_text(&self.selected_species)
                        .show_ui(ui, |ui| {
                            for name in &self.species_names {
                                ui.selectable_value(
                                    &mut self.selected_species,
                                    name.clone(),
                                    name.as_str(),
                                );
                            }
                        });
                    if self.selected_species != prev_species {
                        self.species_changed = Some(self.selected_species.clone());
                    }

                    ui.add_space(4.0);

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

                    ui.horizontal(|ui| {
                        if ui.button("Randomize").clicked() {
                            self.params = PlantParams::randomize();
                            self.dirty = true;
                        }
                        if ui.button("Reset Defaults").clicked() {
                            self.params = PlantParams::default();
                            self.dirty = true;
                        }
                    });

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
