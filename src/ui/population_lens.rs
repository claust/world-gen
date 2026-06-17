//! Population Lens — a live in-game dialogue that reads the typed
//! [`EvolutionRegionReport`] for a chunk-sized region around the camera and
//! makes the plant-evolution simulation legible without pausing it.
//!
//! Phase 2 of `docs/PLANT_EVOLUTION_VISUALIZATION_PLAN.md`: data before
//! decoration. The panel is a pure reader of the Phase 1 report; the app
//! samples the report each frame and hands it in. The only state the panel owns
//! is its visibility and the query radius, so the report always matches what the
//! debug CLI returns for the same center/radius.

use egui::{Color32, RichText, Sense, Stroke};

use crate::world_core::chunk::CHUNK_SIZE_METERS;
use crate::world_core::evolution::{
    EvolutionGeneStats, EvolutionOverlayMode, EvolutionRegionReport,
};

use super::theme;
use super::ui_registry::UiRegistry;

/// Genes in their canonical layout order with display labels, so the panel
/// shows them in a meaningful sequence rather than the report's alphabetical
/// `BTreeMap` order.
const GENE_ROWS: [(&str, &str); 8] = [
    ("wet_pref", "Wet pref"),
    ("alt_pref", "Alt pref"),
    ("stress_width", "Stress width"),
    ("capture", "Capture"),
    ("fecundity", "Fecundity"),
    ("seed_mass", "Seed mass"),
    ("dispersal", "Dispersal"),
    ("timing", "Timing"),
];

/// The overlay buttons, paired with the mode they select and a stable debug id.
const OVERLAY_BUTTONS: [(&str, &str, EvolutionOverlayMode); 6] = [
    ("evo-overlay-off", "Off", EvolutionOverlayMode::Off),
    (
        "evo-overlay-wet",
        "Wet",
        EvolutionOverlayMode::WetPreference,
    ),
    (
        "evo-overlay-alt",
        "Alt",
        EvolutionOverlayMode::AltitudePreference,
    ),
    (
        "evo-overlay-fitness",
        "Fitness",
        EvolutionOverlayMode::AbioticFitness,
    ),
    (
        "evo-overlay-stress",
        "Stress",
        EvolutionOverlayMode::CompetitionStress,
    ),
    ("evo-overlay-gen", "Gen", EvolutionOverlayMode::Generation),
];

const RADIUS_SLIDER_ID: &str = "lens-radius";
const RADIUS_MIN: f32 = 32.0;
const RADIUS_MAX: f32 = 1024.0;

/// How many times to overdraw the opaque background. The egui pass blends over
/// the scene at ~0.6 opacity, so each layer leaves ~0.4 of the previous through;
/// eight layers drives residual scene bleed below ~1% for any plausible factor.
const BG_OVERDRAW: usize = 8;

/// Fully opaque fill, lifted toward a neutral slate so it reads as a distinct
/// solid surface over green terrain rather than vanishing into it (the shared
/// `theme::PANEL_BG` is alpha-200 translucent, and a near-black green blends
/// into shadowed grass). White text stays high-contrast on this.
const PANEL_FILL: Color32 = Color32::from_rgb(24, 32, 28);
/// A bright border so the panel edge is unmistakable even over similar-hued
/// vegetation — the main cue that the panel is solid, not a tint.
const PANEL_STROKE: Color32 = Color32::from_rgb(120, 186, 120);
/// Brighter than `theme::TEXT_SECONDARY` so the numeric readouts stay readable
/// on the dark fill.
const TEXT: Color32 = Color32::from_rgb(198, 212, 198);

pub struct PopulationLens {
    visible: bool,
    radius: f32,
}

impl PopulationLens {
    pub fn new() -> Self {
        Self {
            visible: false,
            radius: CHUNK_SIZE_METERS,
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// The query radius the app should sample the region report at. Read before
    /// `ui()` so the report matches the radius the player last committed; a
    /// slider drag this frame takes effect on the next sample, which is fine for
    /// a live, camera-following lens.
    pub fn radius(&self) -> f32 {
        self.radius
    }

    /// Draw the lens. Returns a new overlay mode if the player picked one this
    /// frame, so the app can apply it to the running world.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        registry: &mut UiRegistry,
        report: &EvolutionRegionReport,
        current_overlay: EvolutionOverlayMode,
    ) -> Option<EvolutionOverlayMode> {
        // Register debug-drivable controls every frame, even while collapsed, so
        // the debug API keeps parity with the keyboard/mouse path.
        registry.register_slider(
            RADIUS_SLIDER_ID,
            "Lens radius",
            self.radius as f64,
            RADIUS_MIN as f64,
            RADIUS_MAX as f64,
        );
        if let Some(v) = registry.consume_set_value(RADIUS_SLIDER_ID) {
            if let Ok(r) = v.parse::<f32>() {
                self.radius = r.clamp(RADIUS_MIN, RADIUS_MAX);
            }
        }
        for (id, label, _) in OVERLAY_BUTTONS {
            registry.register_button(id, label);
        }

        if !self.visible {
            // Still consume any queued overlay clicks so a debug command issued
            // while the panel is hidden isn't left dangling for the next open.
            for (id, _, mode) in OVERLAY_BUTTONS {
                if registry.consume_click(id) {
                    return Some(mode);
                }
            }
            return None;
        }

        let mut overlay_pick = None;
        // Debug-API clicks are processed regardless of egui z-order/layout.
        for (id, _, mode) in OVERLAY_BUTTONS {
            if registry.consume_click(id) {
                overlay_pick = Some(mode);
            }
        }

        egui::Window::new("population_lens")
            .title_bar(false)
            .default_width(320.0)
            .resizable(false)
            // Anchor bottom-right: the one screen corner free of HUD furniture
            // (top-left telemetry, top-right compass, bottom-left minimap). A
            // fixed bottom edge also keeps the panel steady as its height grows
            // and shrinks with the region's plant count.
            .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -12.0])
            // Transparent frame, opaque background painted by hand: the egui
            // pass is composited over the 3D scene at less than full opacity
            // (it draws with `LoadOp::Load` and a single alpha-blended pass), so
            // a normal window fill — even at alpha 255 — lets terrain bleed
            // through. We instead overdraw an opaque rect a few times so the
            // residual scene bleed is negligible, and keep only the frame's
            // border, shadow, and margins. The title bar is disabled so this
            // background covers the whole panel, title included.
            .frame(
                egui::Frame::window(ctx.style().as_ref())
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.5, PANEL_STROKE)),
            )
            .show(ctx, |ui| {
                // Reserve background slots up front so they paint behind content;
                // they're filled once the content rect is known.
                let bg: Vec<_> = (0..BG_OVERDRAW)
                    .map(|_| ui.painter().add(egui::Shape::Noop))
                    .collect();

                ui.label(
                    RichText::new("Population Lens")
                        .size(18.0)
                        .strong()
                        .color(theme::ACCENT),
                );
                self.controls(ui, current_overlay, &mut overlay_pick);
                ui.separator();
                self.population_section(ui, report);
                ui.separator();
                if report.counts.empty {
                    ui.label(RichText::new("No plants in this region.").color(TEXT));
                } else {
                    self.genes_section(ui, report);
                    ui.separator();
                    self.phenotype_section(ui, report);
                }

                let rect = ui.min_rect().expand(6.0);
                for slot in bg {
                    ui.painter().set(
                        slot,
                        egui::Shape::rect_filled(rect, egui::CornerRadius::same(6), PANEL_FILL),
                    );
                }
            });

        overlay_pick
    }

    fn controls(
        &mut self,
        ui: &mut egui::Ui,
        current_overlay: EvolutionOverlayMode,
        overlay_pick: &mut Option<EvolutionOverlayMode>,
    ) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Radius").size(13.0));
            ui.add(
                egui::Slider::new(&mut self.radius, RADIUS_MIN..=RADIUS_MAX)
                    .suffix(" m")
                    .logarithmic(true),
            );
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Overlay").size(13.0));
            for (_, label, mode) in OVERLAY_BUTTONS {
                let selected = mode == current_overlay;
                if ui.selectable_label(selected, label).clicked() {
                    *overlay_pick = Some(mode);
                }
            }
        });
    }

    fn population_section(&self, ui: &mut egui::Ui, report: &EvolutionRegionReport) {
        ui.label(theme::section_header("Population"));
        let s = &report.stages;
        ui.label(
            RichText::new(format!(
                "{} plants  ·  {} chunks",
                report.plant_count, report.counts.chunks_touched
            ))
            .size(14.0)
            .strong()
            .color(Color32::WHITE),
        );
        ui.label(
            RichText::new(format!(
                "seedling {}  ·  young {}  ·  mature {}  ·  dead {}",
                s.seedling, s.young, s.mature, s.dead
            ))
            .size(12.0)
            .color(TEXT),
        );
        ui.label(
            RichText::new(format!(
                "fitness {:.2}  ·  stress {:.2}",
                report.mean_abiotic_fitness, report.mean_competition_stress
            ))
            .size(12.0)
            .color(TEXT),
        );
        if !report.top_species.is_empty() {
            let top: Vec<String> = report
                .top_species
                .iter()
                .take(3)
                .map(|sp| format!("{} ×{}", sp.name, sp.count))
                .collect();
            ui.label(RichText::new(top.join("   ")).size(12.0).color(TEXT));
        }
    }

    fn genes_section(&self, ui: &mut egui::Ui, report: &EvolutionRegionReport) {
        ui.label(theme::section_header("Genes"));
        for (key, label) in GENE_ROWS {
            if let Some(stats) = report.genes.get(key) {
                gene_bar(ui, label, stats);
            }
        }
    }

    fn phenotype_section(&self, ui: &mut egui::Ui, report: &EvolutionRegionReport) {
        ui.label(theme::section_header("Phenotype here"));
        let p = &report.phenotype;
        // Unit-range traits get a bar; the scale traits centre on 1.0 so a plain
        // number reads more clearly than a 0..1 bar would.
        unit_bar(ui, "Fitness", p.abiotic_fitness);
        unit_bar(ui, "Stress", p.stress);
        unit_bar(ui, "Establish", p.establishment_chance);
        ui.label(
            RichText::new(format!(
                "height ×{:.2}  ·  width ×{:.2}  ·  seeds ×{:.2}  ·  spread ×{:.2}",
                p.height_scale, p.width_scale, p.seed_count_scale, p.spread_radius_scale
            ))
            .size(12.0)
            .color(TEXT),
        );
    }
}

impl Default for PopulationLens {
    fn default() -> Self {
        Self::new()
    }
}

const BAR_W: f32 = 150.0;
const BAR_H: f32 = 14.0;
const LABEL_W: f32 = 90.0;
/// Bar track: darker than the panel fill so the inset reads against it.
const TRACK: Color32 = Color32::from_rgb(12, 17, 14);

/// A gene row: a translucent ±1σ variance band over a track, with a bright tick
/// at the mean. Gene stats are byte-valued (0..255), so normalise to 0..1.
fn gene_bar(ui: &mut egui::Ui, label: &str, stats: &EvolutionGeneStats) {
    let mean = (stats.mean / 255.0).clamp(0.0, 1.0) as f32;
    let sd = (stats.stddev / 255.0) as f32;
    let lo = (mean - sd).clamp(0.0, 1.0);
    let hi = (mean + sd).clamp(0.0, 1.0);
    ui.horizontal(|ui| {
        ui.add_sized(
            [LABEL_W, BAR_H],
            egui::Label::new(RichText::new(label).size(13.0).color(TEXT)),
        );
        let (rect, _) = ui.allocate_exact_size(egui::vec2(BAR_W, BAR_H), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, TRACK);
        let band = egui::Rect::from_min_max(
            egui::pos2(rect.left() + lo * rect.width(), rect.top()),
            egui::pos2(rect.left() + hi * rect.width(), rect.bottom()),
        );
        painter.rect_filled(band, 0.0, theme::ACCENT_MUTED.gamma_multiply(0.6));
        let mx = rect.left() + mean * rect.width();
        painter.line_segment(
            [egui::pos2(mx, rect.top()), egui::pos2(mx, rect.bottom())],
            Stroke::new(2.0, theme::ACCENT),
        );
        ui.label(
            RichText::new(format!("{mean:.2}"))
                .size(12.0)
                .strong()
                .color(Color32::WHITE),
        );
    });
}

/// A simple filled bar for a 0..1 phenotype trait.
fn unit_bar(ui: &mut egui::Ui, label: &str, value: f64) {
    let v = (value as f32).clamp(0.0, 1.0);
    ui.horizontal(|ui| {
        ui.add_sized(
            [LABEL_W, BAR_H],
            egui::Label::new(RichText::new(label).size(13.0).color(TEXT)),
        );
        let (rect, _) = ui.allocate_exact_size(egui::vec2(BAR_W, BAR_H), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, TRACK);
        let fill = egui::Rect::from_min_max(
            rect.min,
            egui::pos2(rect.left() + v * rect.width(), rect.bottom()),
        );
        painter.rect_filled(fill, 3.0, theme::ACCENT_MUTED);
        ui.label(
            RichText::new(format!("{v:.2}"))
                .size(12.0)
                .strong()
                .color(Color32::WHITE),
        );
    });
}
