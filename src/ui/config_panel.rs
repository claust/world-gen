use crate::world_core::config::GameConfig;

use super::ui_registry::UiRegistry;

pub struct ConfigPanel {
    visible: bool,
    config: GameConfig,
    last_applied: GameConfig,
    dirty: bool,
}

impl ConfigPanel {
    pub fn new(config: &GameConfig) -> Self {
        Self {
            visible: false,
            config: config.clone(),
            last_applied: config.clone(),
            dirty: false,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Returns the changed config when the pointer is released after changes.
    /// This debounces so chunks don't regenerate mid-drag.
    pub fn take_dirty_config(&mut self, ctx: &egui::Context) -> Option<GameConfig> {
        if !self.dirty {
            return None;
        }
        // Apply on pointer release (debounces during slider drag) or when no pointer
        // is active (handles debug API set_value which doesn't involve the mouse).
        let released = ctx.input(|i| i.pointer.any_released());
        let no_pointer_down = ctx.input(|i| !i.pointer.any_down());
        if released || no_pointer_down {
            self.dirty = false;
            self.last_applied = self.config.clone();
            Some(self.config.clone())
        } else {
            None
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context, registry: &mut UiRegistry) {
        if !self.visible {
            return;
        }

        // Register all elements and process debug actions (always runs, even when sections collapsed)
        self.register_and_consume(registry);

        egui::SidePanel::left("config_panel")
            .default_width(320.0)
            .frame(
                egui::Frame::side_top_panel(ctx.style().as_ref())
                    .fill(egui::Color32::from_rgba_unmultiplied(30, 30, 30, 220)),
            )
            .show(ctx, |ui| {
                ui.heading("World Config");
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Handle section toggle clicks
                    toggle_section(ui, registry, "section-heightmap", "Heightmap");
                    toggle_section(ui, registry, "section-biomes", "Biomes");
                    toggle_section(ui, registry, "section-trees", "Trees");
                    toggle_section(ui, registry, "section-ferns", "Ferns");
                    toggle_section(ui, registry, "section-houses", "Houses");
                    toggle_section(ui, registry, "section-world", "World");

                    self.heightmap_section(ui);
                    self.biome_section(ui);
                    self.flora_section(ui);
                    self.ferns_section(ui);
                    self.houses_section(ui);
                    self.world_section(ui);

                    ui.separator();
                    if ui.button("Reset to Defaults").clicked()
                        || registry.consume_click("btn-reset-defaults")
                    {
                        self.config = GameConfig::default();
                        self.dirty = true;
                    }
                });
            });

        if self.config != self.last_applied {
            self.dirty = true;
        }
    }

    /// Register all UI elements and consume pending debug actions.
    /// Runs every frame regardless of collapsing state.
    fn register_and_consume(&mut self, registry: &mut UiRegistry) {
        // -- Section toggle buttons --
        registry.register_button("section-heightmap", "Heightmap");
        registry.register_button("section-biomes", "Biomes");
        registry.register_button("section-trees", "Trees");
        registry.register_button("section-ferns", "Ferns");
        registry.register_button("section-houses", "Houses");
        registry.register_button("section-world", "World");
        registry.register_button("btn-reset-defaults", "Reset to Defaults");

        // -- Heightmap: Continental --
        {
            let c = &mut self.config.heightmap.continental;
            reg_f64(
                registry,
                "slider-continental-frequency",
                "Continental Frequency",
                &mut c.frequency,
                0.0001,
                0.01,
            );
            reg_f32(
                registry,
                "slider-continental-amplitude",
                "Continental Amplitude",
                &mut c.amplitude,
                10.0,
                300.0,
            );
        }
        // -- Heightmap: Ridge --
        {
            let r = &mut self.config.heightmap.ridge;
            reg_f64(
                registry,
                "slider-ridge-frequency",
                "Ridge Frequency",
                &mut r.frequency,
                0.0001,
                0.05,
            );
            reg_f32(
                registry,
                "slider-ridge-amplitude",
                "Ridge Amplitude",
                &mut r.amplitude,
                0.0,
                200.0,
            );
        }
        // -- Heightmap: Detail --
        {
            let d = &mut self.config.heightmap.detail;
            reg_f64(
                registry,
                "slider-detail-frequency",
                "Detail Frequency",
                &mut d.frequency,
                0.001,
                0.1,
            );
            reg_f32(
                registry,
                "slider-detail-amplitude",
                "Detail Amplitude",
                &mut d.amplitude,
                0.0,
                50.0,
            );
        }
        // -- Heightmap: Moisture --
        {
            let h = &mut self.config.heightmap;
            reg_f64(
                registry,
                "slider-moisture-base-freq",
                "Moisture Base Freq",
                &mut h.moisture_base_frequency,
                0.0001,
                0.01,
            );
            reg_f64(
                registry,
                "slider-moisture-var-freq",
                "Moisture Var Freq",
                &mut h.moisture_variation_frequency,
                0.0001,
                0.05,
            );
            reg_f32(
                registry,
                "slider-moisture-base-weight",
                "Moisture Base Weight",
                &mut h.moisture_base_weight,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-moisture-var-weight",
                "Moisture Var Weight",
                &mut h.moisture_variation_weight,
                0.0,
                1.0,
            );
            reg_f64(
                registry,
                "slider-moisture-var-offset-x",
                "Moisture Var Offset X",
                &mut h.moisture_variation_offset_x,
                -100.0,
                100.0,
            );
            reg_f64(
                registry,
                "slider-moisture-var-offset-z",
                "Moisture Var Offset Z",
                &mut h.moisture_variation_offset_z,
                -100.0,
                100.0,
            );
        }
        // -- Biomes --
        {
            let b = &mut self.config.biome;
            reg_f32(
                registry,
                "slider-snow-height",
                "Snow Height",
                &mut b.snow_height,
                50.0,
                300.0,
            );
            reg_f32(
                registry,
                "slider-rock-height",
                "Rock Height",
                &mut b.rock_height,
                30.0,
                250.0,
            );
            reg_f32(
                registry,
                "slider-desert-moisture",
                "Desert Moisture",
                &mut b.desert_moisture,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-forest-moisture",
                "Forest Moisture",
                &mut b.forest_moisture,
                0.0,
                1.0,
            );
        }
        // -- Trees --
        {
            let f = &mut self.config.flora;
            reg_f32(
                registry,
                "slider-tree-grid-spacing",
                "Tree Grid Spacing",
                &mut f.grid_spacing,
                3.0,
                30.0,
            );
            reg_f32(
                registry,
                "slider-forest-density-base",
                "Forest Density Base",
                &mut f.forest_density_base,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-forest-density-scale",
                "Forest Density Scale",
                &mut f.forest_density_scale,
                0.0,
                2.0,
            );
            reg_f32(
                registry,
                "slider-forest-density-min",
                "Forest Density Min",
                &mut f.forest_density_min,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-forest-density-max",
                "Forest Density Max",
                &mut f.forest_density_max,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-forest-moisture-center",
                "Forest Moisture Center",
                &mut f.forest_moisture_center,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-grassland-density-base",
                "Grassland Density Base",
                &mut f.grassland_density_base,
                0.0,
                0.5,
            );
            reg_f32(
                registry,
                "slider-grassland-density-scale",
                "Grassland Density Scale",
                &mut f.grassland_density_scale,
                0.0,
                0.5,
            );
            reg_f32(
                registry,
                "slider-grassland-density-min",
                "Grassland Density Min",
                &mut f.grassland_density_min,
                0.0,
                0.5,
            );
            reg_f32(
                registry,
                "slider-grassland-density-max",
                "Grassland Density Max",
                &mut f.grassland_density_max,
                0.0,
                0.5,
            );
            reg_f32(
                registry,
                "slider-trunk-height-min",
                "Trunk Height Min",
                &mut f.trunk_height_min,
                1.0,
                15.0,
            );
            reg_f32(
                registry,
                "slider-trunk-height-range",
                "Trunk Height Range",
                &mut f.trunk_height_range,
                0.0,
                20.0,
            );
            reg_f32(
                registry,
                "slider-canopy-radius-min",
                "Canopy Radius Min",
                &mut f.canopy_radius_min,
                0.5,
                5.0,
            );
            reg_f32(
                registry,
                "slider-canopy-radius-range",
                "Canopy Radius Range",
                &mut f.canopy_radius_range,
                0.0,
                5.0,
            );
            reg_f32(
                registry,
                "slider-tree-max-slope",
                "Tree Max Slope",
                &mut f.max_slope,
                0.0,
                3.0,
            );
            reg_f32(
                registry,
                "slider-tree-min-height",
                "Tree Min Height",
                &mut f.min_height,
                -50.0,
                50.0,
            );
        }
        // -- Ferns --
        {
            let f = &mut self.config.ferns;
            reg_f32(
                registry,
                "slider-fern-grid-spacing",
                "Fern Grid Spacing",
                &mut f.grid_spacing,
                1.0,
                15.0,
            );
            reg_f32(
                registry,
                "slider-fern-forest-offset",
                "Fern Forest Offset",
                &mut f.forest_density_offset,
                0.0,
                2.0,
            );
            reg_f32(
                registry,
                "slider-fern-forest-scale",
                "Fern Forest Scale",
                &mut f.forest_density_scale,
                0.0,
                5.0,
            );
            reg_f32(
                registry,
                "slider-fern-forest-max",
                "Fern Forest Max",
                &mut f.forest_density_max,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-fern-grassland-offset",
                "Fern Grassland Offset",
                &mut f.grassland_density_offset,
                0.0,
                2.0,
            );
            reg_f32(
                registry,
                "slider-fern-grassland-scale",
                "Fern Grassland Scale",
                &mut f.grassland_density_scale,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-fern-grassland-max",
                "Fern Grassland Max",
                &mut f.grassland_density_max,
                0.0,
                0.5,
            );
            reg_f32(
                registry,
                "slider-fern-scale-min",
                "Fern Scale Min",
                &mut f.scale_min,
                0.1,
                2.0,
            );
            reg_f32(
                registry,
                "slider-fern-scale-range",
                "Fern Scale Range",
                &mut f.scale_range,
                0.0,
                2.0,
            );
            reg_f32(
                registry,
                "slider-fern-max-slope",
                "Fern Max Slope",
                &mut f.max_slope,
                0.0,
                3.0,
            );
            reg_f32(
                registry,
                "slider-fern-min-height",
                "Fern Min Height",
                &mut f.min_height,
                -50.0,
                50.0,
            );
        }
        // -- Houses --
        {
            let h = &mut self.config.houses;
            reg_f32(
                registry,
                "slider-house-grid-spacing",
                "House Grid Spacing",
                &mut h.grid_spacing,
                10.0,
                100.0,
            );
            reg_f32(
                registry,
                "slider-house-grassland-density",
                "House Grassland Density",
                &mut h.grassland_density,
                0.0,
                0.3,
            );
            reg_f32(
                registry,
                "slider-house-max-slope",
                "House Max Slope",
                &mut h.max_slope,
                0.0,
                1.0,
            );
            reg_f32(
                registry,
                "slider-house-height-min",
                "House Height Min",
                &mut h.height_min,
                -50.0,
                100.0,
            );
            reg_f32(
                registry,
                "slider-house-height-max",
                "House Height Max",
                &mut h.height_max,
                0.0,
                300.0,
            );
        }
        // -- World --
        {
            reg_f32(
                registry,
                "slider-sea-level",
                "Sea Level",
                &mut self.config.sea_level,
                0.0,
                50.0,
            );
            reg_i32(
                registry,
                "slider-load-radius",
                "Load Radius",
                &mut self.config.world.load_radius,
                0,
                5,
            );
            reg_f32(
                registry,
                "slider-day-speed",
                "Day Speed",
                &mut self.config.world.day_speed,
                0.0,
                100.0,
            );
        }
    }

    fn heightmap_section(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Heightmap", |ui| {
            ui.label("Continental");
            let c = &mut self.config.heightmap.continental;
            ui.add(
                egui::Slider::new(&mut c.frequency, 0.0001..=0.01)
                    .text("frequency")
                    .logarithmic(true),
            );
            ui.add(egui::Slider::new(&mut c.amplitude, 10.0..=300.0).text("amplitude"));

            ui.label("Ridge");
            let r = &mut self.config.heightmap.ridge;
            ui.add(
                egui::Slider::new(&mut r.frequency, 0.0001..=0.05)
                    .text("frequency")
                    .logarithmic(true),
            );
            ui.add(egui::Slider::new(&mut r.amplitude, 0.0..=200.0).text("amplitude"));

            ui.label("Detail");
            let d = &mut self.config.heightmap.detail;
            ui.add(
                egui::Slider::new(&mut d.frequency, 0.001..=0.1)
                    .text("frequency")
                    .logarithmic(true),
            );
            ui.add(egui::Slider::new(&mut d.amplitude, 0.0..=50.0).text("amplitude"));

            ui.separator();
            ui.label("Moisture");
            let h = &mut self.config.heightmap;
            ui.add(
                egui::Slider::new(&mut h.moisture_base_frequency, 0.0001..=0.01)
                    .text("base freq")
                    .logarithmic(true),
            );
            ui.add(
                egui::Slider::new(&mut h.moisture_variation_frequency, 0.0001..=0.05)
                    .text("var freq")
                    .logarithmic(true),
            );
            ui.add(egui::Slider::new(&mut h.moisture_base_weight, 0.0..=1.0).text("base weight"));
            ui.add(
                egui::Slider::new(&mut h.moisture_variation_weight, 0.0..=1.0).text("var weight"),
            );
            ui.add(
                egui::Slider::new(&mut h.moisture_variation_offset_x, -100.0..=100.0)
                    .text("var offset X"),
            );
            ui.add(
                egui::Slider::new(&mut h.moisture_variation_offset_z, -100.0..=100.0)
                    .text("var offset Z"),
            );
        });
    }

    fn biome_section(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Biomes", |ui| {
            let b = &mut self.config.biome;
            ui.add(egui::Slider::new(&mut b.snow_height, 50.0..=300.0).text("snow height"));
            ui.add(egui::Slider::new(&mut b.rock_height, 30.0..=250.0).text("rock height"));
            ui.add(egui::Slider::new(&mut b.desert_moisture, 0.0..=1.0).text("desert moisture"));
            ui.add(egui::Slider::new(&mut b.forest_moisture, 0.0..=1.0).text("forest moisture"));
        });
    }

    fn flora_section(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Trees", |ui| {
            let f = &mut self.config.flora;
            ui.add(egui::Slider::new(&mut f.grid_spacing, 3.0..=30.0).text("grid spacing"));

            ui.label("Forest density");
            ui.add(egui::Slider::new(&mut f.forest_density_base, 0.0..=1.0).text("base"));
            ui.add(egui::Slider::new(&mut f.forest_density_scale, 0.0..=2.0).text("scale"));
            ui.add(egui::Slider::new(&mut f.forest_density_min, 0.0..=1.0).text("min"));
            ui.add(egui::Slider::new(&mut f.forest_density_max, 0.0..=1.0).text("max"));
            ui.add(
                egui::Slider::new(&mut f.forest_moisture_center, 0.0..=1.0).text("moisture center"),
            );

            ui.label("Grassland density");
            ui.add(egui::Slider::new(&mut f.grassland_density_base, 0.0..=0.5).text("base"));
            ui.add(egui::Slider::new(&mut f.grassland_density_scale, 0.0..=0.5).text("scale"));
            ui.add(egui::Slider::new(&mut f.grassland_density_min, 0.0..=0.5).text("min"));
            ui.add(egui::Slider::new(&mut f.grassland_density_max, 0.0..=0.5).text("max"));

            ui.label("Tree size");
            ui.add(egui::Slider::new(&mut f.trunk_height_min, 1.0..=15.0).text("trunk height min"));
            ui.add(
                egui::Slider::new(&mut f.trunk_height_range, 0.0..=20.0).text("trunk height range"),
            );
            ui.add(
                egui::Slider::new(&mut f.canopy_radius_min, 0.5..=5.0).text("canopy radius min"),
            );
            ui.add(
                egui::Slider::new(&mut f.canopy_radius_range, 0.0..=5.0)
                    .text("canopy radius range"),
            );

            ui.label("Placement");
            ui.add(egui::Slider::new(&mut f.max_slope, 0.0..=3.0).text("max slope"));
            ui.add(egui::Slider::new(&mut f.min_height, -50.0..=50.0).text("min height"));
        });
    }

    fn ferns_section(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Ferns", |ui| {
            let f = &mut self.config.ferns;
            ui.add(egui::Slider::new(&mut f.grid_spacing, 1.0..=15.0).text("grid spacing"));

            ui.label("Forest density");
            ui.add(egui::Slider::new(&mut f.forest_density_offset, 0.0..=2.0).text("offset"));
            ui.add(egui::Slider::new(&mut f.forest_density_scale, 0.0..=5.0).text("scale"));
            ui.add(egui::Slider::new(&mut f.forest_density_max, 0.0..=1.0).text("max"));

            ui.label("Grassland density");
            ui.add(egui::Slider::new(&mut f.grassland_density_offset, 0.0..=2.0).text("offset"));
            ui.add(egui::Slider::new(&mut f.grassland_density_scale, 0.0..=1.0).text("scale"));
            ui.add(egui::Slider::new(&mut f.grassland_density_max, 0.0..=0.5).text("max"));

            ui.label("Size");
            ui.add(egui::Slider::new(&mut f.scale_min, 0.1..=2.0).text("scale min"));
            ui.add(egui::Slider::new(&mut f.scale_range, 0.0..=2.0).text("scale range"));

            ui.label("Placement");
            ui.add(egui::Slider::new(&mut f.max_slope, 0.0..=3.0).text("max slope"));
            ui.add(egui::Slider::new(&mut f.min_height, -50.0..=50.0).text("min height"));
        });
    }

    fn houses_section(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("Houses", |ui| {
            let h = &mut self.config.houses;
            ui.add(egui::Slider::new(&mut h.grid_spacing, 10.0..=100.0).text("grid spacing"));
            ui.add(
                egui::Slider::new(&mut h.grassland_density, 0.0..=0.3).text("grassland density"),
            );
            ui.add(egui::Slider::new(&mut h.max_slope, 0.0..=1.0).text("max slope"));
            ui.add(egui::Slider::new(&mut h.height_min, -50.0..=100.0).text("height min"));
            ui.add(egui::Slider::new(&mut h.height_max, 0.0..=300.0).text("height max"));
        });
    }

    fn world_section(&mut self, ui: &mut egui::Ui) {
        ui.collapsing("World", |ui| {
            ui.add(egui::Slider::new(&mut self.config.sea_level, 0.0..=50.0).text("sea level"));
            ui.add(
                egui::Slider::new(&mut self.config.world.load_radius, 0..=5).text("load radius"),
            );
            ui.add(
                egui::Slider::new(&mut self.config.world.day_speed, 0.0..=100.0)
                    .text("day speed")
                    .logarithmic(true),
            );
        });
    }
}

/// Toggle a collapsing section's open/closed state via the debug registry.
fn toggle_section(ui: &mut egui::Ui, registry: &mut UiRegistry, btn_id: &str, label: &str) {
    if registry.consume_click(btn_id) {
        let egui_id = ui.make_persistent_id(label);
        // Ensure a collapsing state exists; if none is stored yet, create one with a default.
        let mut state = egui::collapsing_header::CollapsingState::load_with_default(
            ui.ctx(),
            egui_id,
            true,
        );
        let was_open = state.is_open();
        state.set_open(!was_open);
        state.store(ui.ctx());
    }
}

/// Register an f32 slider and consume pending set_value action.
fn reg_f32(registry: &mut UiRegistry, id: &str, label: &str, val: &mut f32, min: f32, max: f32) {
    registry.register_slider(id, label, *val as f64, min as f64, max as f64);
    if let Some(v) = registry.consume_set_value(id) {
        if let Ok(f) = v.parse::<f32>() {
            *val = f.clamp(min, max);
        }
    }
}

/// Register an f64 slider and consume pending set_value action.
fn reg_f64(registry: &mut UiRegistry, id: &str, label: &str, val: &mut f64, min: f64, max: f64) {
    registry.register_slider(id, label, *val, min, max);
    if let Some(v) = registry.consume_set_value(id) {
        if let Ok(f) = v.parse::<f64>() {
            *val = f.clamp(min, max);
        }
    }
}

/// Register an i32 slider and consume pending set_value action.
fn reg_i32(registry: &mut UiRegistry, id: &str, label: &str, val: &mut i32, min: i32, max: i32) {
    registry.register_int_slider(id, label, *val as i64, min as i64, max as i64);
    if let Some(v) = registry.consume_set_value(id) {
        if let Ok(n) = v.parse::<i32>() {
            *val = n.clamp(min, max);
        }
    }
}
