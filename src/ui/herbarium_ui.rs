use egui::{self, Color32, CornerRadius, Pos2, RichText, Sense, Vec2};

use crate::world_core::herbarium::Herbarium;

use super::ui_registry::UiRegistry;

pub enum HerbariumAction {
    OpenPlant(usize),
    NewPlant,
    Back,
}

#[derive(Default)]
pub struct HerbariumUi;

impl HerbariumUi {
    pub fn ui(
        &self,
        ctx: &egui::Context,
        herbarium: &Herbarium,
        registry: &mut UiRegistry,
    ) -> Option<HerbariumAction> {
        let mut action = None;

        egui::CentralPanel::default()
            .frame({
                #[allow(deprecated)]
                egui::Frame::none().fill(Color32::from_rgba_unmultiplied(10, 40, 15, 200))
            })
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);

                    ui.label(
                        RichText::new("Herbarium")
                            .size(42.0)
                            .color(Color32::from_rgb(140, 220, 120)),
                    );
                    ui.add_space(20.0);

                    // Back button
                    registry.register_button("btn-herbarium-back", "Back");
                    let back_btn = ui.add(
                        egui::Button::new(RichText::new("Back").size(16.0).color(Color32::WHITE))
                            .fill(Color32::from_rgba_unmultiplied(30, 80, 30, 180)),
                    );
                    if back_btn.clicked() || registry.consume_click("btn-herbarium-back") {
                        action = Some(HerbariumAction::Back);
                    }
                    ui.add_space(20.0);
                });

                // Tile grid in a scroll area
                let tile_size = Vec2::new(140.0, 160.0);
                let spacing = 16.0;

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let available_width = ui.available_width();
                        let cols = ((available_width + spacing) / (tile_size.x + spacing)).floor()
                            as usize;
                        let cols = cols.max(1);

                        // Center the grid
                        let grid_width =
                            cols as f32 * tile_size.x + (cols - 1).max(0) as f32 * spacing;
                        let left_margin = ((available_width - grid_width) / 2.0).max(0.0);

                        let total_items = herbarium.plants.len() + 1; // +1 for "+" tile
                        let rows = total_items.div_ceil(cols);

                        for row in 0..rows {
                            ui.horizontal(|ui| {
                                ui.add_space(left_margin);
                                for col in 0..cols {
                                    let idx = row * cols + col;
                                    if idx > herbarium.plants.len() {
                                        break;
                                    }

                                    if col > 0 {
                                        ui.add_space(spacing);
                                    }

                                    if idx < herbarium.plants.len() {
                                        // Plant tile
                                        let entry = &herbarium.plants[idx];
                                        let tile_id = format!("btn-herb-plant-{idx}");
                                        registry.register_button(&tile_id, &entry.name);

                                        let (rect, response) =
                                            ui.allocate_exact_size(tile_size, Sense::click());

                                        if ui.is_rect_visible(rect) {
                                            let painter = ui.painter();

                                            // Tile background — derive color from leaf HSL
                                            let leaf = &entry.species.color.leaf;
                                            let tile_color = hsl_to_color32(
                                                leaf.h,
                                                leaf.s * 0.7,
                                                leaf.l * 0.8,
                                                200,
                                            );

                                            let hover = response.hovered();
                                            let bg = if hover {
                                                lighten(tile_color, 30)
                                            } else {
                                                tile_color
                                            };

                                            painter.rect_filled(rect, CornerRadius::same(8), bg);

                                            // Border
                                            painter.rect_stroke(
                                                rect,
                                                CornerRadius::same(8),
                                                egui::Stroke::new(
                                                    if hover { 2.0 } else { 1.0 },
                                                    Color32::from_rgb(140, 220, 120),
                                                ),
                                                egui::StrokeKind::Outside,
                                            );

                                            // Plant name centered
                                            let text_pos =
                                                Pos2::new(rect.center().x, rect.max.y - 28.0);
                                            painter.text(
                                                text_pos,
                                                egui::Align2::CENTER_CENTER,
                                                &entry.name,
                                                egui::FontId::proportional(16.0),
                                                Color32::WHITE,
                                            );

                                            // Small species indicator at top
                                            let species_pos =
                                                Pos2::new(rect.center().x, rect.min.y + 20.0);
                                            painter.text(
                                                species_pos,
                                                egui::Align2::CENTER_CENTER,
                                                &entry.species.name,
                                                egui::FontId::proportional(11.0),
                                                Color32::from_white_alpha(160),
                                            );
                                        }

                                        if response.clicked() || registry.consume_click(&tile_id) {
                                            action = Some(HerbariumAction::OpenPlant(idx));
                                        }
                                    } else {
                                        // "+" new plant tile
                                        registry.register_button("btn-herb-new-plant", "New Plant");

                                        let (rect, response) =
                                            ui.allocate_exact_size(tile_size, Sense::click());

                                        if ui.is_rect_visible(rect) {
                                            let painter = ui.painter();
                                            let hover = response.hovered();

                                            let bg = if hover {
                                                Color32::from_rgba_unmultiplied(50, 100, 50, 200)
                                            } else {
                                                Color32::from_rgba_unmultiplied(30, 70, 30, 180)
                                            };

                                            painter.rect_filled(rect, CornerRadius::same(8), bg);

                                            painter.rect_stroke(
                                                rect,
                                                CornerRadius::same(8),
                                                egui::Stroke::new(
                                                    1.0,
                                                    Color32::from_rgb(100, 180, 90),
                                                ),
                                                egui::StrokeKind::Outside,
                                            );

                                            // Plus sign
                                            painter.text(
                                                rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                "+",
                                                egui::FontId::proportional(48.0),
                                                Color32::from_rgb(140, 220, 120),
                                            );
                                        }

                                        if response.clicked()
                                            || registry.consume_click("btn-herb-new-plant")
                                        {
                                            action = Some(HerbariumAction::NewPlant);
                                        }
                                    }
                                }
                            });
                            ui.add_space(spacing);
                        }
                    });
            });

        action
    }
}

fn hsl_to_color32(h: f32, s: f32, l: f32, alpha: u8) -> Color32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h_prime = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (h_prime % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match h_prime as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        5 => (c, 0.0, x),
        _ => (0.0, 0.0, 0.0),
    };
    let m = l - c / 2.0;
    Color32::from_rgba_unmultiplied(
        ((r1 + m) * 255.0) as u8,
        ((g1 + m) * 255.0) as u8,
        ((b1 + m) * 255.0) as u8,
        alpha,
    )
}

fn lighten(c: Color32, amount: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(
        c.r().saturating_add(amount),
        c.g().saturating_add(amount),
        c.b().saturating_add(amount),
        c.a(),
    )
}
