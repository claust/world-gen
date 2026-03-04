use rand::Rng;
use serde::Serialize;

use crate::world_core::plant_gen::config::SpeciesConfig;

use super::theme;
use super::ui_registry::UiRegistry;

const CROWN_SHAPES: &[&str] = &[
    "conical", "columnar", "dome", "oval", "vase", "umbrella", "weeping", "fan_top",
];
const LENGTH_PROFILES: &[&str] = &["conical", "dome", "columnar", "vase", "layered"];
const FOLIAGE_STYLES: &[&str] = &["broadleaf", "needle", "scale_leaf", "palm_frond", "none"];
const BODY_KINDS: &[&str] = &["tree", "shrub"];
const ARRANGEMENT_TYPES: &[&str] = &["spiral", "opposite", "whorled", "random"];
const CLUSTER_TYPES: &[&str] = &["dense_mass", "clusters", "individual", "ring"];

#[derive(Clone, Serialize, PartialEq)]
pub struct PlantParams {
    // Crown (existing)
    pub crown_shape: String,
    pub length_profile: String,
    pub foliage_style: String,
    pub apical_dominance: f32,
    pub gravity_response: f32,
    pub crown_base: f32,
    pub crown_density: f32,
    pub aspect_ratio: f32,

    // Body Plan
    pub body_kind: String,
    pub stem_count: u32,
    pub max_height_min: f32,
    pub max_height_max: f32,

    // Trunk
    pub taper: f32,
    pub base_flare: f32,
    pub straightness: f32,
    pub thickness_ratio: f32,

    // Branching (new)
    pub max_depth: u32,
    pub arrangement_type: String,
    pub arrangement_angle: f32,
    pub branches_per_node_min: u32,
    pub branches_per_node_max: u32,
    pub insertion_angle_base_min: f32,
    pub insertion_angle_base_max: f32,
    pub insertion_angle_tip_min: f32,
    pub insertion_angle_tip_max: f32,
    pub child_length_ratio: f32,
    pub child_thickness_ratio: f32,
    pub randomness: f32,

    // Crown (new)
    pub asymmetry: f32,

    // Foliage (new)
    pub leaf_size_min: f32,
    pub leaf_size_max: f32,
    pub cluster_type: String,
    pub cluster_count: u32,
    pub droop: f32,
    pub coverage: f32,

    // Color
    pub bark_h: f32,
    pub bark_s: f32,
    pub bark_l: f32,
    pub leaf_h: f32,
    pub leaf_s: f32,
    pub leaf_l: f32,
    pub leaf_variance: f32,
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

            body_kind: "tree".to_string(),
            stem_count: 1,
            max_height_min: 12.0,
            max_height_max: 18.0,

            taper: 0.4,
            base_flare: 0.35,
            straightness: 0.82,
            thickness_ratio: 0.05,

            max_depth: 3,
            arrangement_type: "spiral".to_string(),
            arrangement_angle: 137.5,
            branches_per_node_min: 2,
            branches_per_node_max: 4,
            insertion_angle_base_min: 65.0,
            insertion_angle_base_max: 80.0,
            insertion_angle_tip_min: 35.0,
            insertion_angle_tip_max: 50.0,
            child_length_ratio: 0.65,
            child_thickness_ratio: 0.7,
            randomness: 0.4,

            asymmetry: 0.2,

            leaf_size_min: 0.02,
            leaf_size_max: 0.05,
            cluster_type: "clusters".to_string(),
            cluster_count: 5,
            droop: 0.3,
            coverage: 0.4,

            bark_h: 25.0,
            bark_s: 0.40,
            bark_l: 0.22,
            leaf_h: 115.0,
            leaf_s: 0.50,
            leaf_l: 0.32,
            leaf_variance: 0.15,
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

            body_kind: spec.body_plan.kind.clone(),
            stem_count: spec.body_plan.stem_count,
            max_height_min: spec.body_plan.max_height[0],
            max_height_max: spec.body_plan.max_height[1],

            taper: spec.trunk.taper,
            base_flare: spec.trunk.base_flare,
            straightness: spec.trunk.straightness,
            thickness_ratio: spec.trunk.thickness_ratio,

            max_depth: spec.branching.max_depth,
            arrangement_type: spec.branching.arrangement.kind.clone(),
            arrangement_angle: spec.branching.arrangement.angle.unwrap_or(137.5),
            branches_per_node_min: spec.branching.branches_per_node[0],
            branches_per_node_max: spec.branching.branches_per_node[1],
            insertion_angle_base_min: spec.branching.insertion_angle.base[0],
            insertion_angle_base_max: spec.branching.insertion_angle.base[1],
            insertion_angle_tip_min: spec.branching.insertion_angle.tip[0],
            insertion_angle_tip_max: spec.branching.insertion_angle.tip[1],
            child_length_ratio: spec.branching.child_length_ratio,
            child_thickness_ratio: spec.branching.child_thickness_ratio,
            randomness: spec.branching.randomness,

            asymmetry: spec.crown.asymmetry,

            leaf_size_min: spec.foliage.leaf_size[0],
            leaf_size_max: spec.foliage.leaf_size[1],
            cluster_type: spec.foliage.cluster_strategy.kind.clone(),
            cluster_count: spec.foliage.cluster_strategy.count.unwrap_or(5),
            droop: spec.foliage.droop,
            coverage: spec.foliage.coverage,

            bark_h: spec.color.bark.h,
            bark_s: spec.color.bark.s,
            bark_l: spec.color.bark.l,
            leaf_h: spec.color.leaf.h,
            leaf_s: spec.color.leaf.s,
            leaf_l: spec.color.leaf.l,
            leaf_variance: spec.color.leaf_variance.unwrap_or(0.15),
        }
    }

    pub fn randomize() -> Self {
        let mut rng = rand::rng();
        let max_h_min = rng.random_range(1.0..20.0_f32);
        let leaf_min = rng.random_range(0.005..2.0_f32);
        let ins_base_min = rng.random_range(20.0..70.0_f32);
        let ins_tip_min = rng.random_range(15.0..50.0_f32);
        Self {
            crown_shape: CROWN_SHAPES[rng.random_range(0..CROWN_SHAPES.len())].to_string(),
            length_profile: LENGTH_PROFILES[rng.random_range(0..LENGTH_PROFILES.len())].to_string(),
            foliage_style: FOLIAGE_STYLES[rng.random_range(0..FOLIAGE_STYLES.len())].to_string(),
            apical_dominance: rng.random_range(0.0..1.0),
            gravity_response: rng.random_range(0.0..1.0),
            crown_base: rng.random_range(0.0..0.8),
            crown_density: rng.random_range(0.2..1.0),
            aspect_ratio: rng.random_range(0.5..2.0),

            body_kind: BODY_KINDS[rng.random_range(0..BODY_KINDS.len())].to_string(),
            stem_count: rng.random_range(1..=7),
            max_height_min: max_h_min,
            max_height_max: rng.random_range(max_h_min..30.0),

            taper: rng.random_range(0.1..0.6),
            base_flare: rng.random_range(0.0..0.5),
            straightness: rng.random_range(0.5..1.0),
            thickness_ratio: rng.random_range(0.02..0.08),

            max_depth: rng.random_range(0..=4),
            arrangement_type: ARRANGEMENT_TYPES[rng.random_range(0..ARRANGEMENT_TYPES.len())]
                .to_string(),
            arrangement_angle: rng.random_range(90.0..180.0),
            branches_per_node_min: rng.random_range(1..=3),
            branches_per_node_max: rng.random_range(3..=6),
            insertion_angle_base_min: ins_base_min,
            insertion_angle_base_max: rng.random_range(ins_base_min..90.0),
            insertion_angle_tip_min: ins_tip_min,
            insertion_angle_tip_max: rng.random_range(ins_tip_min..70.0),
            child_length_ratio: rng.random_range(0.4..0.8),
            child_thickness_ratio: rng.random_range(0.3..0.8),
            randomness: rng.random_range(0.0..0.6),

            asymmetry: rng.random_range(0.0..0.4),

            leaf_size_min: leaf_min,
            leaf_size_max: rng.random_range(leaf_min..5.0),
            cluster_type: CLUSTER_TYPES[rng.random_range(0..CLUSTER_TYPES.len())].to_string(),
            cluster_count: rng.random_range(3..=16),
            droop: rng.random_range(0.0..0.8),
            coverage: rng.random_range(0.2..1.0),

            bark_h: rng.random_range(15.0..45.0),
            bark_s: rng.random_range(0.2..0.5),
            bark_l: rng.random_range(0.15..0.35),
            leaf_h: rng.random_range(80.0..160.0),
            leaf_s: rng.random_range(0.3..0.7),
            leaf_l: rng.random_range(0.18..0.4),
            leaf_variance: rng.random_range(0.05..0.3),
        }
    }
}

pub enum EditorAction {
    Back,
    Delete,
    #[cfg(not(target_arch = "wasm32"))]
    Screenshot,
}

#[derive(Default)]
pub struct PlantEditorPanel {
    params: PlantParams,
    last_applied: PlantParams,
    /// The initial params for the current species, used by "Reset Defaults".
    initial_params: PlantParams,
    dirty: bool,
}

impl PlantEditorPanel {
    /// Push new params from outside (e.g. after a species preset change).
    pub fn set_params(&mut self, params: PlantParams) {
        self.params = params.clone();
        self.last_applied = params.clone();
        self.initial_params = params;
        self.dirty = false;
    }

    pub fn current_params(&self) -> &PlantParams {
        &self.params
    }

    /// Returns changed params when the pointer is released after changes.
    /// Debounces so the tree doesn't regenerate mid-drag. Also applies
    /// immediately when no pointer is active (handles debug API set_value).
    pub fn take_dirty_params(&mut self, ctx: &egui::Context) -> Option<PlantParams> {
        if !self.dirty {
            return None;
        }
        let released = ctx.input(|i| i.pointer.any_released());
        let no_pointer_down = ctx.input(|i| !i.pointer.any_down());
        if released || no_pointer_down {
            self.dirty = false;
            self.last_applied = self.params.clone();
            Some(self.params.clone())
        } else {
            None
        }
    }

    /// Draw the plant editor side panel. Returns an action if Back or Delete was clicked.
    pub fn ui(&mut self, ctx: &egui::Context, registry: &mut UiRegistry) -> Option<EditorAction> {
        let mut action = None;

        egui::SidePanel::left("plant_editor_panel")
            .default_width(400.0)
            .frame(egui::Frame::side_top_panel(ctx.style().as_ref()).fill(theme::PANEL_BG))
            .show(ctx, |ui| {
                // Back button — top-left, fixed square size
                ui.add_space(4.0);
                let back_size = egui::vec2(theme::BACK_BUTTON_SIZE, theme::BACK_BUTTON_SIZE);
                registry.register_button("btn-back-to-herbarium", "Back to Herbarium");
                if ui.add_sized(back_size, theme::back_button()).clicked()
                    || registry.consume_click("btn-back-to-herbarium")
                {
                    action = Some(EditorAction::Back);
                }
                ui.add_space(4.0);
                ui.label(theme::title("Plant Editor", theme::PANEL_TITLE_SIZE));
                ui.add_space(4.0);
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    // --- Body Plan ---
                    egui::CollapsingHeader::new(theme::section_header("Body Plan"))
                        .default_open(false)
                        .show(ui, |ui| {
                            registry.register_combo(
                                "combo-body-kind",
                                "Kind",
                                &self.params.body_kind,
                                BODY_KINDS,
                            );
                            if let Some(v) = registry.consume_set_value("combo-body-kind") {
                                if BODY_KINDS.contains(&v.as_str()) {
                                    self.params.body_kind = v;
                                }
                            }
                            dropdown(ui, "Kind", &mut self.params.body_kind, BODY_KINDS);

                            reg_int(
                                registry,
                                "slider-stem-count",
                                "Stem Count",
                                &mut self.params.stem_count,
                                1,
                                10,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.stem_count, 1..=10)
                                    .text("Stem Count"),
                            );

                            reg_f32(
                                registry,
                                "slider-height-min",
                                "Height Min",
                                &mut self.params.max_height_min,
                                0.5,
                                30.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.max_height_min, 0.5..=30.0)
                                    .text("Height Min"),
                            );
                            if self.params.max_height_max < self.params.max_height_min {
                                self.params.max_height_max = self.params.max_height_min;
                            }
                            reg_f32(
                                registry,
                                "slider-height-max",
                                "Height Max",
                                &mut self.params.max_height_max,
                                0.5,
                                30.0,
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.params.max_height_max,
                                    self.params.max_height_min..=30.0,
                                )
                                .text("Height Max"),
                            );
                        });

                    // --- Trunk ---
                    egui::CollapsingHeader::new(theme::section_header("Trunk"))
                        .default_open(false)
                        .show(ui, |ui| {
                            reg_f32(
                                registry,
                                "slider-taper",
                                "Taper",
                                &mut self.params.taper,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.taper, 0.0..=1.0).text("Taper"),
                            );
                            reg_f32(
                                registry,
                                "slider-base-flare",
                                "Base Flare",
                                &mut self.params.base_flare,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.base_flare, 0.0..=1.0)
                                    .text("Base Flare"),
                            );
                            reg_f32(
                                registry,
                                "slider-straightness",
                                "Straightness",
                                &mut self.params.straightness,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.straightness, 0.0..=1.0)
                                    .text("Straightness"),
                            );
                            reg_f32(
                                registry,
                                "slider-thickness-ratio",
                                "Thickness Ratio",
                                &mut self.params.thickness_ratio,
                                0.01,
                                0.15,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.thickness_ratio, 0.01..=0.15)
                                    .text("Thickness Ratio"),
                            );
                        });

                    // --- Crown ---
                    egui::CollapsingHeader::new(theme::section_header("Crown"))
                        .default_open(true)
                        .show(ui, |ui| {
                            registry.register_combo(
                                "combo-crown-shape",
                                "Crown Shape",
                                &self.params.crown_shape,
                                CROWN_SHAPES,
                            );
                            if let Some(v) = registry.consume_set_value("combo-crown-shape") {
                                if CROWN_SHAPES.contains(&v.as_str()) {
                                    self.params.crown_shape = v;
                                }
                            }
                            dropdown(
                                ui,
                                "Crown Shape",
                                &mut self.params.crown_shape,
                                CROWN_SHAPES,
                            );
                            reg_f32(
                                registry,
                                "slider-crown-base",
                                "Crown Base",
                                &mut self.params.crown_base,
                                0.0,
                                0.8,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.crown_base, 0.0..=0.8)
                                    .text("Crown Base"),
                            );
                            reg_f32(
                                registry,
                                "slider-aspect-ratio",
                                "Aspect Ratio",
                                &mut self.params.aspect_ratio,
                                0.5,
                                2.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.aspect_ratio, 0.5..=2.0)
                                    .text("Aspect Ratio"),
                            );
                            reg_f32(
                                registry,
                                "slider-crown-density",
                                "Crown Density",
                                &mut self.params.crown_density,
                                0.2,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.crown_density, 0.2..=1.0)
                                    .text("Density"),
                            );
                            reg_f32(
                                registry,
                                "slider-asymmetry",
                                "Asymmetry",
                                &mut self.params.asymmetry,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.asymmetry, 0.0..=1.0)
                                    .text("Asymmetry"),
                            );
                        });

                    // --- Branching ---
                    egui::CollapsingHeader::new(theme::section_header("Branching"))
                        .default_open(false)
                        .show(ui, |ui| {
                            registry.register_combo(
                                "combo-length-profile",
                                "Length Profile",
                                &self.params.length_profile,
                                LENGTH_PROFILES,
                            );
                            if let Some(v) = registry.consume_set_value("combo-length-profile") {
                                if LENGTH_PROFILES.contains(&v.as_str()) {
                                    self.params.length_profile = v;
                                }
                            }
                            dropdown(
                                ui,
                                "Length Profile",
                                &mut self.params.length_profile,
                                LENGTH_PROFILES,
                            );
                            reg_f32(
                                registry,
                                "slider-apical-dominance",
                                "Apical Dominance",
                                &mut self.params.apical_dominance,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.apical_dominance, 0.0..=1.0)
                                    .text("Apical Dominance"),
                            );
                            reg_f32(
                                registry,
                                "slider-gravity-response",
                                "Gravity Response",
                                &mut self.params.gravity_response,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.gravity_response, 0.0..=1.0)
                                    .text("Gravity Response"),
                            );
                            reg_int(
                                registry,
                                "slider-max-depth",
                                "Max Depth",
                                &mut self.params.max_depth,
                                0,
                                5,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.max_depth, 0..=5)
                                    .text("Max Depth"),
                            );
                            registry.register_combo(
                                "combo-arrangement",
                                "Arrangement",
                                &self.params.arrangement_type,
                                ARRANGEMENT_TYPES,
                            );
                            if let Some(v) = registry.consume_set_value("combo-arrangement") {
                                if ARRANGEMENT_TYPES.contains(&v.as_str()) {
                                    self.params.arrangement_type = v;
                                }
                            }
                            dropdown(
                                ui,
                                "Arrangement",
                                &mut self.params.arrangement_type,
                                ARRANGEMENT_TYPES,
                            );
                            if self.params.arrangement_type == "spiral" {
                                reg_f32(
                                    registry,
                                    "slider-spiral-angle",
                                    "Spiral Angle",
                                    &mut self.params.arrangement_angle,
                                    0.0,
                                    360.0,
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut self.params.arrangement_angle,
                                        0.0..=360.0,
                                    )
                                    .text("Spiral Angle"),
                                );
                            }
                            reg_int(
                                registry,
                                "slider-branches-per-node-min",
                                "Branches/Node Min",
                                &mut self.params.branches_per_node_min,
                                0,
                                6,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.branches_per_node_min, 0..=6)
                                    .text("Branches/Node Min"),
                            );
                            if self.params.branches_per_node_max < self.params.branches_per_node_min
                            {
                                self.params.branches_per_node_max =
                                    self.params.branches_per_node_min;
                            }
                            reg_int(
                                registry,
                                "slider-branches-per-node-max",
                                "Branches/Node Max",
                                &mut self.params.branches_per_node_max,
                                0,
                                6,
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.params.branches_per_node_max,
                                    self.params.branches_per_node_min..=6,
                                )
                                .text("Branches/Node Max"),
                            );
                            reg_f32(
                                registry,
                                "slider-ins-angle-base-min",
                                "Ins. Angle Base Min",
                                &mut self.params.insertion_angle_base_min,
                                0.0,
                                90.0,
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.params.insertion_angle_base_min,
                                    0.0..=90.0,
                                )
                                .text("Ins. Angle Base Min"),
                            );
                            if self.params.insertion_angle_base_max
                                < self.params.insertion_angle_base_min
                            {
                                self.params.insertion_angle_base_max =
                                    self.params.insertion_angle_base_min;
                            }
                            reg_f32(
                                registry,
                                "slider-ins-angle-base-max",
                                "Ins. Angle Base Max",
                                &mut self.params.insertion_angle_base_max,
                                0.0,
                                90.0,
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.params.insertion_angle_base_max,
                                    self.params.insertion_angle_base_min..=90.0,
                                )
                                .text("Ins. Angle Base Max"),
                            );
                            reg_f32(
                                registry,
                                "slider-ins-angle-tip-min",
                                "Ins. Angle Tip Min",
                                &mut self.params.insertion_angle_tip_min,
                                0.0,
                                90.0,
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.params.insertion_angle_tip_min,
                                    0.0..=90.0,
                                )
                                .text("Ins. Angle Tip Min"),
                            );
                            if self.params.insertion_angle_tip_max
                                < self.params.insertion_angle_tip_min
                            {
                                self.params.insertion_angle_tip_max =
                                    self.params.insertion_angle_tip_min;
                            }
                            reg_f32(
                                registry,
                                "slider-ins-angle-tip-max",
                                "Ins. Angle Tip Max",
                                &mut self.params.insertion_angle_tip_max,
                                0.0,
                                90.0,
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.params.insertion_angle_tip_max,
                                    self.params.insertion_angle_tip_min..=90.0,
                                )
                                .text("Ins. Angle Tip Max"),
                            );
                            reg_f32(
                                registry,
                                "slider-child-length-ratio",
                                "Child Length Ratio",
                                &mut self.params.child_length_ratio,
                                0.1,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.child_length_ratio, 0.1..=1.0)
                                    .text("Child Length Ratio"),
                            );
                            reg_f32(
                                registry,
                                "slider-child-thickness-ratio",
                                "Child Thickness Ratio",
                                &mut self.params.child_thickness_ratio,
                                0.1,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.params.child_thickness_ratio,
                                    0.1..=1.0,
                                )
                                .text("Child Thickness Ratio"),
                            );
                            reg_f32(
                                registry,
                                "slider-randomness",
                                "Randomness",
                                &mut self.params.randomness,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.randomness, 0.0..=1.0)
                                    .text("Randomness"),
                            );
                        });

                    // --- Foliage ---
                    egui::CollapsingHeader::new(theme::section_header("Foliage"))
                        .default_open(false)
                        .show(ui, |ui| {
                            registry.register_combo(
                                "combo-foliage-style",
                                "Foliage Style",
                                &self.params.foliage_style,
                                FOLIAGE_STYLES,
                            );
                            if let Some(v) = registry.consume_set_value("combo-foliage-style") {
                                if FOLIAGE_STYLES.contains(&v.as_str()) {
                                    self.params.foliage_style = v;
                                }
                            }
                            dropdown(
                                ui,
                                "Foliage Style",
                                &mut self.params.foliage_style,
                                FOLIAGE_STYLES,
                            );
                            reg_f32(
                                registry,
                                "slider-leaf-size-min",
                                "Leaf Size Min",
                                &mut self.params.leaf_size_min,
                                0.005,
                                5.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.leaf_size_min, 0.005..=5.0)
                                    .text("Leaf Size Min"),
                            );
                            if self.params.leaf_size_max < self.params.leaf_size_min {
                                self.params.leaf_size_max = self.params.leaf_size_min;
                            }
                            reg_f32(
                                registry,
                                "slider-leaf-size-max",
                                "Leaf Size Max",
                                &mut self.params.leaf_size_max,
                                0.005,
                                5.0,
                            );
                            ui.add(
                                egui::Slider::new(
                                    &mut self.params.leaf_size_max,
                                    self.params.leaf_size_min..=5.0,
                                )
                                .text("Leaf Size Max"),
                            );
                            registry.register_combo(
                                "combo-cluster-type",
                                "Cluster Type",
                                &self.params.cluster_type,
                                CLUSTER_TYPES,
                            );
                            if let Some(v) = registry.consume_set_value("combo-cluster-type") {
                                if CLUSTER_TYPES.contains(&v.as_str()) {
                                    self.params.cluster_type = v;
                                }
                            }
                            dropdown(
                                ui,
                                "Cluster Type",
                                &mut self.params.cluster_type,
                                CLUSTER_TYPES,
                            );
                            if self.params.cluster_type == "clusters"
                                || self.params.cluster_type == "ring"
                            {
                                reg_int(
                                    registry,
                                    "slider-cluster-count",
                                    "Cluster Count",
                                    &mut self.params.cluster_count,
                                    1,
                                    20,
                                );
                                ui.add(
                                    egui::Slider::new(&mut self.params.cluster_count, 1..=20)
                                        .text("Cluster Count"),
                                );
                            }
                            reg_f32(
                                registry,
                                "slider-droop",
                                "Droop",
                                &mut self.params.droop,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.droop, 0.0..=1.0).text("Droop"),
                            );
                            reg_f32(
                                registry,
                                "slider-coverage",
                                "Coverage",
                                &mut self.params.coverage,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.coverage, 0.0..=1.0)
                                    .text("Coverage"),
                            );
                        });

                    // --- Color ---
                    egui::CollapsingHeader::new(theme::section_header("Color"))
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.label("Bark");
                            reg_f32(
                                registry,
                                "slider-bark-hue",
                                "Bark Hue",
                                &mut self.params.bark_h,
                                0.0,
                                360.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.bark_h, 0.0..=360.0).text("Hue"),
                            );
                            reg_f32(
                                registry,
                                "slider-bark-saturation",
                                "Bark Saturation",
                                &mut self.params.bark_s,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.bark_s, 0.0..=1.0)
                                    .text("Saturation"),
                            );
                            reg_f32(
                                registry,
                                "slider-bark-lightness",
                                "Bark Lightness",
                                &mut self.params.bark_l,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.bark_l, 0.0..=1.0)
                                    .text("Lightness"),
                            );
                            ui.add_space(4.0);
                            ui.label("Leaf");
                            reg_f32(
                                registry,
                                "slider-leaf-hue",
                                "Leaf Hue",
                                &mut self.params.leaf_h,
                                0.0,
                                360.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.leaf_h, 0.0..=360.0).text("Hue"),
                            );
                            reg_f32(
                                registry,
                                "slider-leaf-saturation",
                                "Leaf Saturation",
                                &mut self.params.leaf_s,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.leaf_s, 0.0..=1.0)
                                    .text("Saturation"),
                            );
                            reg_f32(
                                registry,
                                "slider-leaf-lightness",
                                "Leaf Lightness",
                                &mut self.params.leaf_l,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.leaf_l, 0.0..=1.0)
                                    .text("Lightness"),
                            );
                            reg_f32(
                                registry,
                                "slider-leaf-variance",
                                "Leaf Variance",
                                &mut self.params.leaf_variance,
                                0.0,
                                1.0,
                            );
                            ui.add(
                                egui::Slider::new(&mut self.params.leaf_variance, 0.0..=1.0)
                                    .text("Leaf Variance"),
                            );
                        });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);

                    let btn_width = ui.available_width() * 0.8;
                    let btn_size = egui::vec2(btn_width, 36.0);

                    registry.register_button("btn-randomize", "Randomize");
                    registry.register_button("btn-reset-defaults-editor", "Reset Defaults");

                    ui.vertical_centered(|ui| {
                        if ui
                            .add_sized(btn_size, theme::menu_button("Randomize"))
                            .clicked()
                            || registry.consume_click("btn-randomize")
                        {
                            self.params = PlantParams::randomize();
                            self.dirty = true;
                        }

                        ui.add_space(6.0);

                        if ui
                            .add_sized(btn_size, theme::menu_button("Reset Defaults"))
                            .clicked()
                            || registry.consume_click("btn-reset-defaults-editor")
                        {
                            self.params = self.initial_params.clone();
                            self.dirty = true;
                        }

                        ui.add_space(6.0);

                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            registry.register_button("btn-screenshot-plant", "Save Screenshot");
                            if ui
                                .add_sized(btn_size, theme::menu_button("Save Screenshot"))
                                .clicked()
                                || registry.consume_click("btn-screenshot-plant")
                            {
                                action = Some(EditorAction::Screenshot);
                            }

                            ui.add_space(6.0);
                        }

                        registry.register_button("btn-delete-plant", "Delete Plant");
                        if ui
                            .add_sized(btn_size, theme::danger_button("Delete Plant"))
                            .clicked()
                            || registry.consume_click("btn-delete-plant")
                        {
                            action = Some(EditorAction::Delete);
                        }
                    });
                });
            });

        if self.params != self.last_applied {
            self.dirty = true;
        }

        action
    }
}

/// Register an f32 slider and consume any pending set-value action.
fn reg_f32(registry: &mut UiRegistry, id: &str, label: &str, val: &mut f32, min: f32, max: f32) {
    registry.register_slider(id, label, *val as f64, min as f64, max as f64);
    if let Some(v) = registry.consume_set_value(id) {
        if let Ok(f) = v.parse::<f32>() {
            *val = f.clamp(min, max);
        }
    }
}

/// Register a u32 slider and consume any pending set-value action.
fn reg_int(registry: &mut UiRegistry, id: &str, label: &str, val: &mut u32, min: u32, max: u32) {
    registry.register_int_slider(id, label, *val as i64, min as i64, max as i64);
    if let Some(v) = registry.consume_set_value(id) {
        if let Ok(f) = v.parse::<u32>() {
            *val = f.clamp(min, max);
        }
    }
}

/// Helper to draw a dropdown combo box for a string field with a fixed set of options.
fn dropdown(ui: &mut egui::Ui, label: &str, value: &mut String, options: &[&str]) {
    egui::ComboBox::from_label(label)
        .selected_text(value.as_str())
        .show_ui(ui, |ui| {
            for &opt in options {
                ui.selectable_value(value, opt.to_string(), opt);
            }
        });
}
