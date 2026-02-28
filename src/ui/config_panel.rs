use crate::world_core::config::GameConfig;

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
        let released = ctx.input(|i| i.pointer.any_released());
        if released {
            self.dirty = false;
            self.last_applied = self.config.clone();
            Some(self.config.clone())
        } else {
            None
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context) {
        if !self.visible {
            return;
        }

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
                    self.heightmap_section(ui);
                    self.biome_section(ui);
                    self.flora_section(ui);
                    self.ferns_section(ui);
                    self.houses_section(ui);
                    self.world_section(ui);

                    ui.separator();
                    if ui.button("Reset to Defaults").clicked() {
                        self.config = GameConfig::default();
                        self.dirty = true;
                    }
                });
            });

        if self.config != self.last_applied {
            self.dirty = true;
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
