//! Agency MVP — the player's first verbs on top of the evolution sim.
//!
//! Step 01 of `docs/GAMEPLAY.html`: turn the spectator into a player with three
//! moment-to-moment verbs and a reason to use them.
//!
//! - **Plant** (`T`) a seedling of the selected species where the camera looks.
//! - **Cull** (`C`) every plant in a small radius under the crosshair.
//! - **Cycle** (`R`) which species the plant verb sows.
//!
//! A small HUD shows the selected species, the worldwide biomass/biodiversity
//! score (from the census), and three "expeditions" — simple objectives that
//! reward exercising the new verbs. All of it is deliberately thin; depth (seed
//! bank, cross-pollination, the trait tree) comes in later steps.

use egui::{Color32, Pos2, RichText, Stroke};

use crate::world_core::gameplay::EcoScore;

use super::AppState;

/// Metres cleared around the crosshair by one cull. Comfortably larger than a
/// single plant so a cull reliably removes what the player is aiming at, but
/// well under a chunk so it stays a local edit.
const CULL_RADIUS: f32 = 6.0;

/// Ray-march step and reach for the "look at the ground" pick. One metre keeps
/// the hit point within a plant's footprint; 800 m covers the visible fly-cam
/// range without marching to the horizon.
const PICK_STEP: f32 = 1.0;
const PICK_MAX_DIST: f32 = 800.0;

// HUD background, mirroring the Population Lens: the egui pass composites over
// the 3D scene at partial opacity, so an opaque fill is overdrawn a few times to
// kill residual terrain bleed.
const BG_OVERDRAW: usize = 8;
const PANEL_FILL: Color32 = Color32::from_rgb(24, 32, 28);
const PANEL_STROKE: Color32 = Color32::from_rgb(120, 186, 120);
const TEXT: Color32 = Color32::from_rgb(198, 212, 198);
const ACCENT: Color32 = Color32::from_rgb(143, 227, 111);
const DONE: Color32 = Color32::from_rgb(224, 182, 74);

/// Persistent player-agency state: which species the plant verb sows and the
/// running tallies the expeditions score against. Session-scoped for the MVP —
/// planted plants themselves persist in the world's spread delta, but these
/// counters reset on relaunch.
#[derive(Default)]
pub(super) struct GameState {
    pub(super) selected_species: u8,
    pub(super) seeds_planted: u32,
    pub(super) plants_culled: u32,
}

/// One HUD objective row: a title, current progress, and its target.
struct Expedition {
    title: &'static str,
    progress: u32,
    target: u32,
}

impl Expedition {
    fn done(&self) -> bool {
        self.progress >= self.target
    }
}

/// The three MVP expeditions, evaluated against the running action tallies. They
/// reward using the new verbs so the loop teaches itself.
fn expeditions(game: &GameState) -> [Expedition; 3] {
    [
        Expedition {
            title: "First Sprout — plant a seed",
            progress: game.seeds_planted.min(1),
            target: 1,
        },
        Expedition {
            title: "Green Thumb — plant 10 seeds",
            progress: game.seeds_planted,
            target: 10,
        },
        Expedition {
            title: "Natural Selection — cull 20 plants",
            progress: game.plants_culled,
            target: 20,
        },
    ]
}

impl AppState {
    /// World position where the camera's forward ray meets the terrain, marched
    /// against the height field. `None` when the camera looks at the sky or no
    /// ground is within reach. This is the MVP stand-in for a cursor raycast: the
    /// cursor is captured for mouselook in gameplay, so the crosshair *is* the
    /// cursor.
    pub(super) fn pick_ground_ahead(&self) -> Option<(f32, f32)> {
        let world = self.world.as_ref()?;
        let origin = self.camera.position;
        let dir = self.camera.forward();

        let mut t = 0.0f32;
        let mut prev_gap = origin.y - world.sample_height(origin.x, origin.z);
        while t < PICK_MAX_DIST {
            t += PICK_STEP;
            let p = origin + dir * t;
            let gap = p.y - world.sample_height(p.x, p.z);
            if gap <= 0.0 {
                // Crossed below terrain between the last step and this one; place
                // the hit where the vertical gap reaches zero.
                let denom = prev_gap - gap;
                let frac = if denom > 0.0 { prev_gap / denom } else { 0.0 };
                let hit = origin + dir * (t - PICK_STEP + frac * PICK_STEP);
                return Some((hit.x, hit.z));
            }
            prev_gap = gap;
        }
        None
    }

    /// Plant the selected species where the camera looks, with on-screen feedback.
    pub(super) fn plant_at_crosshair(&mut self) {
        let Some((x, z)) = self.pick_ground_ahead() else {
            self.set_game_toast("No ground in view to plant on".to_string());
            return;
        };
        let species = self.game.selected_species;
        let name = self.species_name(species);
        match self.plant_species_at(x, z, species) {
            Ok(()) => self.set_game_toast(format!("Planted {name}")),
            Err(err) => self.set_game_toast(format!("Can't plant {name}: {err}")),
        }
    }

    /// Cull every plant in a small radius where the camera looks.
    pub(super) fn cull_at_crosshair(&mut self) {
        let Some((x, z)) = self.pick_ground_ahead() else {
            self.set_game_toast("No ground in view to cull".to_string());
            return;
        };
        let removed = self.cull_plants_at(x, z, CULL_RADIUS);
        if removed > 0 {
            let plural = if removed == 1 { "" } else { "s" };
            self.set_game_toast(format!("Culled {removed} plant{plural}"));
        } else {
            self.set_game_toast("Nothing to cull here".to_string());
        }
    }

    /// Switch the plant verb to the next registered species, wrapping around.
    pub(super) fn cycle_planting_species(&mut self) {
        let count = self.species_count();
        if count == 0 {
            return;
        }
        self.game.selected_species = ((self.game.selected_species as usize + 1) % count) as u8;
        let name = self.species_name(self.game.selected_species);
        self.set_game_toast(format!("Selected {name}"));
    }

    /// Plant one seedling and, on success, credit the player's tally. Shared by
    /// the crosshair verb and the debug API so both paths score identically.
    pub(super) fn plant_species_at(&mut self, x: f32, z: f32, species: u8) -> Result<(), String> {
        let result = match self.world.as_mut() {
            Some(world) => world.plant_seed(x, z, species).map_err(str::to_string),
            None => Err("world not ready".to_string()),
        };
        if result.is_ok() {
            self.game.seeds_planted += 1;
        }
        result
    }

    /// Cull plants in a radius and credit the player's tally. Returns the count.
    pub(super) fn cull_plants_at(&mut self, x: f32, z: f32, radius: f32) -> usize {
        let removed = self
            .world
            .as_mut()
            .map(|world| world.cull_at(x, z, radius))
            .unwrap_or(0);
        self.game.plants_culled += removed as u32;
        removed
    }

    fn species_count(&self) -> usize {
        self.world.as_ref().map(|w| w.species_count()).unwrap_or(0)
    }

    fn species_name(&self, species: u8) -> String {
        self.world
            .as_ref()
            .and_then(|w| w.species_name(species))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("species {species}"))
    }

    /// Reuse the favorite toast as transient gameplay feedback (it already fades
    /// itself out via `draw_favorite_toast`).
    fn set_game_toast(&mut self, message: String) {
        self.favorite_toast = Some((message, self.elapsed_seconds));
    }

    /// JSON payload describing the current gameplay state, for the debug API.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn game_stats_json(&self) -> serde_json::Value {
        let score = self.world.as_ref().and_then(|w| w.eco_score());
        let exps: Vec<serde_json::Value> = expeditions(&self.game)
            .iter()
            .map(|e| {
                serde_json::json!({
                    "title": e.title,
                    "progress": e.progress,
                    "target": e.target,
                    "done": e.done(),
                })
            })
            .collect();
        serde_json::json!({
            "selected_species": self.game.selected_species,
            "selected_species_name": self.species_name(self.game.selected_species),
            "seeds_planted": self.game.seeds_planted,
            "plants_culled": self.game.plants_culled,
            "eco_score": score.map(|s| serde_json::json!({
                "living": s.living,
                "biomass": s.biomass,
                "species_present": s.species_present,
                "shannon": s.shannon,
            })),
            "expeditions": exps,
        })
    }

    /// Snapshot the data the HUD needs, so the actual drawing can be a free
    /// function. Collected *before* the egui `run` closure: a `&self` method call
    /// inside that closure would force it to capture all of `self` (Rust 2021
    /// disjoint capture), conflicting with the `egui_bridge` receiver the closure
    /// is built on.
    pub(super) fn gameplay_hud_data(&self) -> HudData {
        let exps = expeditions(&self.game);
        HudData {
            species_name: self.species_name(self.game.selected_species),
            score: self.world.as_ref().and_then(|w| w.eco_score()),
            cursor_captured: self.cursor_captured,
            expeditions: exps
                .iter()
                .map(|e| ExpeditionView {
                    title: e.title,
                    progress: e.progress.min(e.target),
                    target: e.target,
                    done: e.done(),
                })
                .collect(),
        }
    }
}

/// Everything [`draw_gameplay_hud`] renders, snapshotted from `AppState`.
pub(super) struct HudData {
    species_name: String,
    score: Option<EcoScore>,
    cursor_captured: bool,
    expeditions: Vec<ExpeditionView>,
}

struct ExpeditionView {
    title: &'static str,
    progress: u32,
    target: u32,
    done: bool,
}

/// Draw the gameplay HUD (top-centre) and, while the cursor is captured, a
/// centre-screen crosshair marking where the plant/cull verbs act. A free
/// function (not a method) so it can be called inside the egui `run` closure
/// without capturing `self`.
pub(super) fn draw_gameplay_hud(ctx: &egui::Context, hud: &HudData) {
    if hud.cursor_captured {
        draw_crosshair(ctx);
    }

    egui::Window::new("gameplay_hud")
        .title_bar(false)
        .resizable(false)
        // Top-centre: the top-left (population counter), top-right (compass) and
        // bottom-left (minimap) corners are occupied by the GPU HUD pass, which
        // composites over egui and would hide this panel there.
        .anchor(egui::Align2::CENTER_TOP, [0.0, 12.0])
        .frame(
            egui::Frame::window(ctx.style().as_ref())
                .fill(Color32::TRANSPARENT)
                .stroke(Stroke::new(1.5, PANEL_STROKE)),
        )
        .show(ctx, |ui| {
            let bg: Vec<_> = (0..BG_OVERDRAW)
                .map(|_| ui.painter().add(egui::Shape::Noop))
                .collect();

            ui.set_max_width(260.0);
            ui.label(
                RichText::new("FloraForge")
                    .size(17.0)
                    .strong()
                    .color(ACCENT),
            );
            ui.label(
                RichText::new(format!("Seeding: {}", hud.species_name))
                    .size(14.0)
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.label(
                RichText::new("[T] plant   [C] cull   [R] species")
                    .size(12.0)
                    .color(TEXT),
            );

            ui.separator();
            if let Some(s) = hud.score {
                ui.label(
                    RichText::new(format!("Biomass  {:.0}", s.biomass))
                        .size(14.0)
                        .color(Color32::WHITE),
                );
                ui.label(
                    RichText::new(format!(
                        "Biodiversity  {} species · H {:.2}",
                        s.species_present, s.shannon
                    ))
                    .size(13.0)
                    .color(TEXT),
                );
                ui.label(
                    RichText::new(format!("Living plants  {}", s.living))
                        .size(13.0)
                        .color(TEXT),
                );
            } else {
                ui.label(RichText::new("Biomass  (sampling…)").size(13.0).color(TEXT));
            }

            ui.separator();
            ui.label(
                RichText::new("Expeditions")
                    .size(13.0)
                    .strong()
                    .color(ACCENT),
            );
            for e in &hud.expeditions {
                let (mark, color) = if e.done { ("✔", DONE) } else { ("•", TEXT) };
                ui.label(
                    RichText::new(format!("{mark} {}  ({}/{})", e.title, e.progress, e.target))
                        .size(12.5)
                        .color(color),
                );
            }

            let rect = ui.min_rect().expand(6.0);
            for slot in bg {
                ui.painter().set(
                    slot,
                    egui::Shape::rect_filled(rect, egui::CornerRadius::same(6), PANEL_FILL),
                );
            }
        });
}

fn draw_crosshair(ctx: &egui::Context) {
    let center = ctx.content_rect().center();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("gameplay-crosshair"),
    ));
    let color = Color32::from_rgba_unmultiplied(255, 255, 255, 190);
    let stroke = Stroke::new(1.5, color);
    let arm = 7.0;
    painter.line_segment(
        [
            Pos2::new(center.x - arm, center.y),
            Pos2::new(center.x + arm, center.y),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(center.x, center.y - arm),
            Pos2::new(center.x, center.y + arm),
        ],
        stroke,
    );
}
