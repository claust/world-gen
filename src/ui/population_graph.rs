//! Population Graph — a worldwide, time-series companion to the Population Lens.
//!
//! Where the lens reads a region around the camera *right now*, this panel plots
//! the whole world's plant counts *over the run*: one line per growth stage, with
//! a species filter so the player can watch all species together or drill into a
//! single one. It is a pure reader of [`PopulationHistory`]; the runtime owns the
//! sampling, this owns only the filter selection.
//!
//! The chart is hand-painted with the egui painter (no plotting dependency),
//! matching the bar-drawing style the lens already uses, and sits just left of
//! the lens so the two read as one HUD cluster when `L` is pressed.

use egui::{Color32, FontId, Pos2, RichText, Sense, Stroke};

use crate::world_core::population_history::{PopulationHistory, STAGE_COUNT, STAGE_LABELS};

use super::theme;
use super::ui_registry::UiRegistry;

/// Debug id for the "all species" filter button.
const FILTER_ALL_ID: &str = "popgraph-species-all";

/// Per-species filter id, e.g. `popgraph-species-oak`, so the debug CLI can drive
/// the filter by name for scripted screenshots.
fn filter_id(name: &str) -> String {
    format!("popgraph-species-{}", name.to_lowercase())
}

/// See [`super::population_lens`] — the egui pass composites at ~0.6 opacity over
/// the 3D scene, so the opaque panel fill is overdrawn this many times to drive
/// residual terrain bleed below ~1%.
const BG_OVERDRAW: usize = 8;
const PANEL_FILL: Color32 = Color32::from_rgb(24, 32, 28);
const PANEL_STROKE: Color32 = Color32::from_rgb(120, 186, 120);
const TEXT: Color32 = Color32::from_rgb(198, 212, 198);
/// Plot inner background — darker than the panel so the chart area reads as an
/// inset, matching the lens's bar tracks.
const PLOT_BG: Color32 = Color32::from_rgb(12, 17, 14);
const GRID: Color32 = Color32::from_rgb(40, 52, 44);

/// One distinct line colour per growth stage, in [`STAGE_LABELS`] order. Greens
/// for the living stages, a blue for mature so it stands out as the headline
/// line, and a weathered tan for dead snags.
const STAGE_COLORS: [Color32; STAGE_COUNT] = [
    Color32::from_rgb(180, 230, 120), // Seedling — light yellow-green
    Color32::from_rgb(90, 195, 95),   // Young — green
    Color32::from_rgb(96, 178, 232),  // Mature — sky blue
    Color32::from_rgb(193, 142, 110), // Dead — tan
];

const PLOT_HEIGHT: f32 = 150.0;

pub struct PopulationGraph {
    /// Species filter: `None` plots every species summed together (the default),
    /// `Some(id)` isolates one species by its census row index.
    selected: Option<usize>,
}

impl Default for PopulationGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl PopulationGraph {
    pub fn new() -> Self {
        Self { selected: None }
    }

    /// Draw the graph. `species_names` is indexed by species id (the census row
    /// index); the panel shows it beside the lens whenever the lens is open.
    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        registry: &mut UiRegistry,
        history: &PopulationHistory,
        species_names: &[String],
    ) {
        // Register debug-drivable filter buttons every frame so the debug API can
        // switch the filter (e.g. for scripted screenshots) in lock-step with the
        // mouse path. Clamp a stale selection if the species set ever shrinks.
        registry.register_button(FILTER_ALL_ID, "All species");
        for name in species_names {
            registry.register_button(&filter_id(name), name);
        }
        if let Some(idx) = self.selected {
            if idx >= species_names.len() {
                self.selected = None;
            }
        }
        if registry.consume_click(FILTER_ALL_ID) {
            self.selected = None;
        }
        for (idx, name) in species_names.iter().enumerate() {
            if registry.consume_click(&filter_id(name)) {
                self.selected = Some(idx);
            }
        }

        egui::Window::new("population_graph")
            .title_bar(false)
            .default_width(360.0)
            .resizable(false)
            // Sit just left of the lens (which anchors bottom-right at -12), with
            // bottom edges aligned so the two panels read as one cluster.
            .anchor(egui::Align2::RIGHT_BOTTOM, [-360.0, -12.0])
            .frame(
                egui::Frame::window(ctx.style().as_ref())
                    .fill(Color32::TRANSPARENT)
                    .stroke(Stroke::new(1.5, PANEL_STROKE)),
            )
            .show(ctx, |ui| {
                let bg: Vec<_> = (0..BG_OVERDRAW)
                    .map(|_| ui.painter().add(egui::Shape::Noop))
                    .collect();

                ui.label(
                    RichText::new("Population over time")
                        .size(18.0)
                        .strong()
                        .color(theme::ACCENT),
                );
                self.filter_row(ui, species_names);
                ui.separator();
                self.plot(ui, history);
                ui.separator();
                self.legend(ui, history);

                let rect = ui.min_rect().expand(6.0);
                for slot in bg {
                    ui.painter().set(
                        slot,
                        egui::Shape::rect_filled(rect, egui::CornerRadius::same(6), PANEL_FILL),
                    );
                }
            });
    }

    fn filter_row(&mut self, ui: &mut egui::Ui, species_names: &[String]) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Species").size(13.0).color(TEXT));
            if ui
                .selectable_label(self.selected.is_none(), "All")
                .clicked()
            {
                self.selected = None;
            }
            for (idx, name) in species_names.iter().enumerate() {
                if ui
                    .selectable_label(self.selected == Some(idx), name)
                    .clicked()
                {
                    self.selected = Some(idx);
                }
            }
        });
    }

    /// The four per-stage series for the current filter: `series[stage]` is a
    /// list of `(sim_hour, count)` points over the retained samples.
    fn series(&self, history: &PopulationHistory) -> [Vec<(f64, f64)>; STAGE_COUNT] {
        let mut series: [Vec<(f64, f64)>; STAGE_COUNT] = Default::default();
        for sample in history.samples() {
            let counts: [u64; STAGE_COUNT] = match self.selected {
                None => sample.stage_totals(),
                Some(id) => sample.species_stages(id).map(|c| c as u64),
            };
            for (stage, &count) in counts.iter().enumerate() {
                series[stage].push((sample.hour, count as f64));
            }
        }
        series
    }

    fn plot(&self, ui: &mut egui::Ui, history: &PopulationHistory) {
        let width = ui.available_width().max(120.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, PLOT_HEIGHT), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, PLOT_BG);
        painter.rect_stroke(rect, 3.0, Stroke::new(1.0, GRID), egui::StrokeKind::Inside);

        if history.is_empty() {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Collecting census data…",
                FontId::proportional(13.0),
                TEXT,
            );
            return;
        }

        let series = self.series(history);
        let (hmin, hmax) = history.span_hours().unwrap_or((0.0, 1.0));
        let span = (hmax - hmin).max(f64::EPSILON);
        // Y axis: 0..max count with 10% headroom, never zero-height.
        let max_count = series
            .iter()
            .flat_map(|s| s.iter().map(|&(_, c)| c))
            .fold(0.0_f64, f64::max);
        let y_top = (max_count * 1.1).max(1.0);

        // Horizontal gridlines + y labels at 0, mid, top.
        for frac in [0.0, 0.5, 1.0] {
            let y = rect.bottom() - frac as f32 * rect.height();
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(1.0, GRID),
            );
            painter.text(
                Pos2::new(rect.left() + 3.0, y - 1.0),
                egui::Align2::LEFT_BOTTOM,
                fmt_count((y_top * frac).round() as u64),
                FontId::proportional(10.0),
                TEXT,
            );
        }

        let project = |hour: f64, count: f64| -> Pos2 {
            let tx = ((hour - hmin) / span) as f32;
            let ty = (count / y_top) as f32;
            Pos2::new(
                rect.left() + tx * rect.width(),
                rect.bottom() - ty * rect.height(),
            )
        };

        let single_point = history.len() == 1;
        for (stage, points) in series.iter().enumerate() {
            let color = STAGE_COLORS[stage];
            let projected: Vec<Pos2> = points.iter().map(|&(h, c)| project(h, c)).collect();
            if single_point {
                if let Some(&p) = projected.first() {
                    painter.circle_filled(p, 2.5, color);
                }
            } else {
                painter.add(egui::Shape::line(projected, Stroke::new(1.8, color)));
            }
        }

        // X axis labels: elapsed days at the start and end of the window.
        painter.text(
            Pos2::new(rect.left() + 2.0, rect.bottom() + 1.0),
            egui::Align2::LEFT_TOP,
            format!("day {:.0}", hmin / 24.0),
            FontId::proportional(10.0),
            TEXT,
        );
        painter.text(
            Pos2::new(rect.right() - 2.0, rect.bottom() + 1.0),
            egui::Align2::RIGHT_TOP,
            format!("day {:.0}", hmax / 24.0),
            FontId::proportional(10.0),
            TEXT,
        );
        // Reserve the row the x labels are painted into so they don't overlap the
        // separator below.
        ui.allocate_space(egui::vec2(width, 12.0));
    }

    fn legend(&self, ui: &mut egui::Ui, history: &PopulationHistory) {
        let latest: [u64; STAGE_COUNT] = match (history.latest(), self.selected) {
            (Some(s), None) => s.stage_totals(),
            (Some(s), Some(id)) => s.species_stages(id).map(|c| c as u64),
            (None, _) => [0; STAGE_COUNT],
        };
        let total: u64 = latest.iter().sum();
        ui.label(
            RichText::new(format!("{} plants now", fmt_count(total)))
                .size(13.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.horizontal_wrapped(|ui| {
            for stage in 0..STAGE_COUNT {
                swatch(ui, STAGE_COLORS[stage]);
                ui.label(
                    RichText::new(format!(
                        "{} {}",
                        STAGE_LABELS[stage],
                        fmt_count(latest[stage])
                    ))
                    .size(12.0)
                    .color(TEXT),
                );
                ui.add_space(6.0);
            }
        });
        if let Some((lo, hi)) = history.span_hours() {
            ui.label(
                RichText::new(format!(
                    "day {:.0}–{:.0}  ·  {} samples",
                    lo / 24.0,
                    hi / 24.0,
                    history.len()
                ))
                .size(11.0)
                .color(theme::TEXT_SECONDARY),
            );
        }
    }
}

/// A small filled colour square for a legend entry.
fn swatch(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
    ui.painter().rect_filled(rect, 2.0, color);
}

/// Format a count with thousands separators, e.g. `12345` → `12,345`.
fn fmt_count(n: u64) -> String {
    let digits = n.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_thousands_separators() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(42), "42");
        assert_eq!(fmt_count(1_000), "1,000");
        assert_eq!(fmt_count(12_345), "12,345");
        assert_eq!(fmt_count(1_234_567), "1,234,567");
    }

    #[test]
    fn filter_id_is_lowercased_and_prefixed() {
        assert_eq!(filter_id("Oak"), "popgraph-species-oak");
        assert_eq!(filter_id("Cattail"), "popgraph-species-cattail");
    }
}
