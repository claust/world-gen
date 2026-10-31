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
//! score (from the census), and "expeditions" — simple objectives that reward
//! exercising the verbs.
//!
//! Step 02 (the breeder loop) adds:
//!
//! - **Pollinate** (`G`): first press gathers pollen from the mature plant under
//!   the crosshair; the next press on a mature plant of the same species crosses
//!   them, sowing a hybrid brood that founds a new strain.
//! - **Found strain** (`F`): tag the plant under the crosshair as the founder of
//!   a named strain, inherited down its maternal line by every seed it spreads.
//! - A strains panel and trait expeditions scored from live strain reports.

use egui::{Color32, Pos2, RichText, Stroke};

use crate::world_core::evolution::PlantGenes;
use crate::world_core::gameplay::EcoScore;
use crate::world_core::strains::{StrainReport, DISPERSAL};
use crate::world_runtime::{Pollen, WILD_STRAIN};

use super::AppState;

/// How far from a debug-API position the plant verbs reach for a plant: the
/// caller names a spot, not an exact trunk.
#[cfg(not(target_arch = "wasm32"))]
pub(super) const PICK_REACH: f32 = 4.0;

/// Reach around a plant the crosshair ray hit: just enough to re-find that
/// exact plant by position, so a neighbour can never be chosen instead.
const AIM_REACH: f32 = 0.25;

/// Reach around the ground hit when the ray passes no plant — catches a
/// seedling too small to intersect while aiming straight at it.
const GROUND_REACH: f32 = 1.5;

/// Strain reports rescan each strain's chunks, so the HUD refreshes them on a
/// timer rather than every frame.
const STRAIN_REFRESH_SECS: f32 = 1.0;

/// The crosshair read-out ray-marches the height field (up to `PICK_MAX_DIST`
/// noise samples), so it is re-taken at 10 Hz rather than every frame.
const TARGET_REFRESH_SECS: f32 = 0.1;

/// Strains listed in the HUD panel, most populous first.
const HUD_STRAIN_ROWS: usize = 4;

/// Living members a strain needs before its trait means count toward an
/// expedition, so one lucky seedling can't complete a breeding goal.
const TRAIT_MIN_LIVING: u32 = 10;

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
    /// Pollen gathered by `G`, waiting to be crossed onto a mother plant.
    pub(super) pollen: Option<Pollen>,
    pub(super) crosses: u32,
    pub(super) strains_founded: u32,
    /// Latest strain reports and when they were taken (`elapsed_seconds`).
    pub(super) strain_reports: Vec<StrainReport>,
    reports_taken_at: Option<f32>,
    /// Cached crosshair read-out and when it was taken.
    target: Option<String>,
    target_taken_at: Option<f32>,
}

impl GameState {
    /// Force the next HUD refresh to re-take the strain reports.
    pub(super) fn invalidate_strain_reports(&mut self) {
        self.reports_taken_at = None;
    }
}

/// One HUD objective row: a title, current progress, and its target.
struct Expedition {
    title: &'static str,
    progress: u32,
    target: u32,
}

/// Best value of `metric` over strains with at least `min_living` members.
fn best_strain(game: &GameState, min_living: u32, metric: impl Fn(&StrainReport) -> u32) -> u32 {
    game.strain_reports
        .iter()
        .filter(|r| r.living >= min_living)
        .map(metric)
        .max()
        .unwrap_or(0)
}

impl Expedition {
    fn done(&self) -> bool {
        self.progress >= self.target
    }
}

/// The expeditions, evaluated against the running action tallies and the live
/// strain reports. The first few reward using each verb so the loop teaches
/// itself; the breeding goals then ask for an outcome selection has to produce.
fn expeditions(game: &GameState) -> [Expedition; 7] {
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
        Expedition {
            title: "Matchmaker — cross-pollinate two plants",
            progress: game.crosses,
            target: 1,
        },
        Expedition {
            title: "Lineage — grow a strain to 50 plants",
            progress: best_strain(game, 0, |r| r.living),
            target: 50,
        },
        Expedition {
            title: "Wanderer — breed a strain to 70% dispersal",
            progress: best_strain(game, TRAIT_MIN_LIVING, |r| {
                (r.mean_genes[DISPERSAL] * 100.0).round() as u32
            }),
            target: 70,
        },
        Expedition {
            title: "Pioneer — a strain thriving in 2 biomes",
            progress: best_strain(game, TRAIT_MIN_LIVING, |r| r.biomes.len() as u32),
            target: 2,
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
        self.ground_hit_ahead().map(|(_, x, z)| (x, z))
    }

    /// As [`pick_ground_ahead`](Self::pick_ground_ahead), also returning the
    /// ray distance to the hit.
    fn ground_hit_ahead(&self) -> Option<(f32, f32, f32)> {
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
                let hit_t = t - PICK_STEP + frac * PICK_STEP;
                let hit = origin + dir * hit_t;
                return Some((hit_t, hit.x, hit.z));
            }
            prev_gap = gap;
        }
        None
    }

    /// The plant under the crosshair, as a position plus the reach that
    /// re-finds it: the first plant the camera ray enters before the terrain
    /// occludes it, else whatever stands right at the ground hit.
    fn aim_at_plant(&self) -> Option<(f32, f32, f32)> {
        let world = self.world.as_ref()?;
        let ground = self.ground_hit_ahead();
        let max_t = ground.map_or(PICK_MAX_DIST, |(t, _, _)| t);
        if let Some((_, x, z)) =
            world.pick_plant_on_ray(self.camera.position, self.camera.forward(), max_t)
        {
            return Some((x, z, AIM_REACH));
        }
        ground.map(|(_, x, z)| (x, z, GROUND_REACH))
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

    /// `G`: gather pollen from the plant under the crosshair, or — holding
    /// pollen — cross it onto that plant. Pollen of another species can't
    /// fertilise the target, so it is swapped for the target's pollen instead;
    /// that doubles as the way to drop held pollen.
    pub(super) fn pollinate_at_crosshair(&mut self) {
        let Some((x, z, reach)) = self.aim_at_plant() else {
            self.set_game_toast("No plant in view".to_string());
            return;
        };
        let message = match self.game.pollen {
            Some(pollen) => match self.cross_at(x, z, reach, pollen) {
                Ok(message) => message,
                Err(err) => match self.world.as_ref().map(|w| w.collect_pollen(x, z, reach)) {
                    Some(Ok(other)) if other.species != pollen.species => {
                        self.game.pollen = Some(other);
                        format!("Swapped to {} pollen", self.species_name(other.species))
                    }
                    _ => format!("Can't cross: {err}"),
                },
            },
            None => match self.gather_pollen_at(x, z, reach) {
                Ok(pollen) => format!(
                    "Gathered {} pollen{} — [G] on a mature plant to cross",
                    self.species_name(pollen.species),
                    self.strain_suffix(pollen.strain),
                ),
                Err(err) => format!("Can't gather pollen: {err}"),
            },
        };
        self.set_game_toast(message);
    }

    /// `F`: found a strain on the plant under the crosshair.
    pub(super) fn found_strain_at_crosshair(&mut self) {
        let Some((x, z, reach)) = self.aim_at_plant() else {
            self.set_game_toast("No plant in view".to_string());
            return;
        };
        let message = match self.found_strain_at(x, z, reach) {
            Ok(name) => format!("Founded the {name} strain"),
            Err(err) => format!("Can't found a strain: {err}"),
        };
        self.set_game_toast(message);
    }

    /// Gather pollen into the player's hand. Shared with the debug API.
    pub(super) fn gather_pollen_at(
        &mut self,
        x: f32,
        z: f32,
        reach: f32,
    ) -> Result<Pollen, String> {
        let world = self.world.as_ref().ok_or("world not ready")?;
        let pollen = world.collect_pollen(x, z, reach)?;
        self.game.pollen = Some(pollen);
        Ok(pollen)
    }

    /// Cross held `pollen` onto the plant at a position, consuming it on
    /// success and crediting the tally. Shared with the debug API.
    pub(super) fn cross_at(
        &mut self,
        x: f32,
        z: f32,
        reach: f32,
        pollen: Pollen,
    ) -> Result<String, String> {
        let world = self.world.as_mut().ok_or("world not ready")?;
        let outcome = world.cross_pollinate(x, z, reach, pollen)?;
        let name = world
            .strain(outcome.strain)
            .map(|s| s.name.clone())
            .unwrap_or_default();
        self.game.pollen = None;
        self.game.crosses += 1;
        self.game.invalidate_strain_reports();
        Ok(format!("Sowed {} {name} hybrids", outcome.seedlings))
    }

    /// Found a strain at a position, returning its name. Shared with the debug
    /// API.
    pub(super) fn found_strain_at(&mut self, x: f32, z: f32, reach: f32) -> Result<String, String> {
        let world = self.world.as_mut().ok_or("world not ready")?;
        let id = world.found_strain(x, z, reach)?;
        self.game.strains_founded += 1;
        self.game.invalidate_strain_reports();
        Ok(world.strain(id).map(|s| s.name.clone()).unwrap_or_default())
    }

    /// Re-take the HUD's throttled reads — strain reports, and the crosshair
    /// read-out — once they are older than their refresh interval (or a verb
    /// invalidated them).
    pub(super) fn refresh_hud_caches(&mut self) {
        let now = self.elapsed_seconds;
        let stale = |taken: Option<f32>, every: f32| taken.is_none_or(|t| now - t >= every);
        if stale(self.game.reports_taken_at, STRAIN_REFRESH_SECS) {
            if let Some(world) = self.world.as_mut() {
                self.game.strain_reports = world.strain_reports();
                self.game.reports_taken_at = Some(now);
            }
        }
        if stale(self.game.target_taken_at, TARGET_REFRESH_SECS) {
            self.game.target = self.target_line();
            self.game.target_taken_at = Some(now);
        }
    }

    fn strain_suffix(&self, strain: u16) -> String {
        if strain == WILD_STRAIN {
            return String::new();
        }
        self.world
            .as_ref()
            .and_then(|w| w.strain(strain))
            .map(|s| format!(" ({})", s.name))
            .unwrap_or_default()
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
            "pollen": self.game.pollen.map(|p| serde_json::json!({
                "species": p.species,
                "species_name": self.species_name(p.species),
                "strain": p.strain,
                "genes": p.genes.to_bytes(),
            })),
            "crosses": self.game.crosses,
            "strains_founded": self.game.strains_founded,
            "strains": strain_reports_json(&self.game.strain_reports),
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
        let mut strains: Vec<StrainRow> = self
            .game
            .strain_reports
            .iter()
            .filter(|r| !r.extinct())
            .map(|r| StrainRow {
                name: r.name.clone(),
                living: r.living,
                dispersal: r.mean_genes[DISPERSAL],
                biomes: r.biomes.len(),
            })
            .collect();
        strains.sort_by(|a, b| b.living.cmp(&a.living));
        let extinct_strains = self.game.strain_reports.len() - strains.len();
        strains.truncate(HUD_STRAIN_ROWS);
        HudData {
            pollen: self.game.pollen.map(|p| {
                format!(
                    "{}{}",
                    self.species_name(p.species),
                    self.strain_suffix(p.strain)
                )
            }),
            target: self.game.target.clone(),
            strains,
            extinct_strains,
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

impl AppState {
    /// One-line read-out of the plant under the crosshair — its species, stage,
    /// strain and the traits the breeding goals ask for — so the player can
    /// choose parents by eye.
    fn target_line(&self) -> Option<String> {
        if !self.cursor_captured {
            return None;
        }
        let (x, z, reach) = self.aim_at_plant()?;
        let pick = self.world.as_ref()?.nearest_plant(x, z, reach)?;
        let p = pick.plant;
        let stage = ["seedling", "young", "mature", "dead"]
            .get(p.stage as usize)
            .copied()
            .unwrap_or("?");
        let pct = |g: u8| (PlantGenes::unit(g) * 100.0).round() as u32;
        Some(format!(
            "{}{} · {stage}\ndisp {}% · fec {}% · hardy {}%",
            self.species_name(p.species),
            self.strain_suffix(p.strain),
            pct(p.genes.dispersal),
            pct(p.genes.fecundity),
            pct(p.genes.stress_width),
        ))
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn strain_reports_json(reports: &[StrainReport]) -> serde_json::Value {
    use crate::world_core::strains::GENE_NAMES;
    reports
        .iter()
        .map(|r| {
            let genes: serde_json::Map<String, serde_json::Value> = GENE_NAMES
                .iter()
                .zip(r.mean_genes)
                .map(|(name, v)| (name.to_string(), serde_json::json!(v)))
                .collect();
            serde_json::json!({
                "id": r.id,
                "name": r.name,
                "species": r.species,
                "hybrid": r.hybrid,
                "living": r.living,
                "stages": r.stages,
                "mean_genes": genes,
                "biomes": r.biomes,
            })
        })
        .collect()
}

struct StrainRow {
    name: String,
    living: u32,
    dispersal: f32,
    biomes: usize,
}

/// Everything [`draw_gameplay_hud`] renders, snapshotted from `AppState`.
pub(super) struct HudData {
    pollen: Option<String>,
    target: Option<String>,
    strains: Vec<StrainRow>,
    extinct_strains: usize,
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

            ui.set_max_width(300.0);
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
                RichText::new(
                    "[T] plant   [C] cull   [R] species\n[G] pollen / cross   [F] found strain",
                )
                .size(12.0)
                .color(TEXT),
            );
            if let Some(pollen) = &hud.pollen {
                ui.label(
                    RichText::new(format!("Holding pollen: {pollen}"))
                        .size(13.0)
                        .color(DONE),
                );
            }
            if let Some(target) = &hud.target {
                ui.label(RichText::new(target).size(12.5).color(Color32::WHITE));
            }

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

            if !hud.strains.is_empty() || hud.extinct_strains > 0 {
                ui.separator();
                ui.label(RichText::new("Strains").size(13.0).strong().color(ACCENT));
                for row in &hud.strains {
                    let biomes = if row.biomes == 1 { "biome" } else { "biomes" };
                    ui.label(
                        RichText::new(format!(
                            "{}  {} · disp {:.0}% · {} {biomes}",
                            row.name,
                            row.living,
                            row.dispersal * 100.0,
                            row.biomes
                        ))
                        .size(12.5)
                        .color(TEXT),
                    );
                }
                if hud.extinct_strains > 0 {
                    ui.label(
                        RichText::new(format!("{} extinct", hud.extinct_strains))
                            .size(12.0)
                            .color(TEXT),
                    );
                }
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
