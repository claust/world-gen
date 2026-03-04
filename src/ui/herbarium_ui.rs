use egui::{self, Color32, CornerRadius, Pos2, Rect, Sense, Vec2};

use crate::renderer_wgpu::thumbnail::ThumbnailRenderer;
use crate::world_core::herbarium::Herbarium;

use super::theme;
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
        thumbnails: Option<&ThumbnailRenderer>,
    ) -> Option<HerbariumAction> {
        let mut action = None;

        egui::CentralPanel::default()
            .frame({
                #[allow(deprecated)]
                egui::Frame::none().fill(theme::PANEL_BG)
            })
            .show(ctx, |ui| {
                // Back button — top-left, fixed square size
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let back_size = egui::vec2(theme::BACK_BUTTON_SIZE, theme::BACK_BUTTON_SIZE);
                    registry.register_button("btn-herbarium-back", "Back");
                    if ui.add_sized(back_size, theme::back_button()).clicked()
                        || registry.consume_click("btn-herbarium-back")
                    {
                        action = Some(HerbariumAction::Back);
                    }
                });
                ui.vertical_centered(|ui| {
                    ui.add_space(10.0);
                    ui.label(theme::title("Herbarium", theme::TITLE_SIZE));
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
                                            let tile_color = theme::hsl_to_color32(
                                                leaf.h,
                                                leaf.s * 0.7,
                                                leaf.l * 0.8,
                                                200,
                                            );

                                            let hover = response.hovered();
                                            let bg = if hover {
                                                theme::lighten(tile_color, 30)
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
                                                    theme::ACCENT,
                                                ),
                                                egui::StrokeKind::Outside,
                                            );

                                            // Thumbnail image in the middle area
                                            if let Some(tex_id) =
                                                thumbnails.and_then(|t| t.get_texture_id(idx))
                                            {
                                                let thumb_size = 100.0;
                                                let thumb_rect = Rect::from_center_size(
                                                    Pos2::new(
                                                        rect.center().x,
                                                        rect.min.y + 35.0 + thumb_size / 2.0,
                                                    ),
                                                    Vec2::splat(thumb_size),
                                                );
                                                painter.image(
                                                    tex_id,
                                                    thumb_rect,
                                                    Rect::from_min_max(
                                                        egui::pos2(0.0, 0.0),
                                                        egui::pos2(1.0, 1.0),
                                                    ),
                                                    Color32::WHITE,
                                                );
                                            }

                                            // Plant name centered
                                            let text_pos =
                                                Pos2::new(rect.center().x, rect.max.y - 16.0);
                                            painter.text(
                                                text_pos,
                                                egui::Align2::CENTER_CENTER,
                                                &entry.name,
                                                egui::FontId::proportional(16.0),
                                                Color32::WHITE,
                                            );

                                            // Small species indicator at top
                                            let species_pos =
                                                Pos2::new(rect.center().x, rect.min.y + 14.0);
                                            painter.text(
                                                species_pos,
                                                egui::Align2::CENTER_CENTER,
                                                &entry.species.name,
                                                egui::FontId::proportional(11.0),
                                                theme::TEXT_SECONDARY,
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
                                                theme::TILE_BG_HOVER
                                            } else {
                                                theme::TILE_BG
                                            };

                                            painter.rect_filled(rect, CornerRadius::same(8), bg);

                                            painter.rect_stroke(
                                                rect,
                                                CornerRadius::same(8),
                                                egui::Stroke::new(1.0, theme::ACCENT_MUTED),
                                                egui::StrokeKind::Outside,
                                            );

                                            // Plus sign
                                            painter.text(
                                                rect.center(),
                                                egui::Align2::CENTER_CENTER,
                                                "+",
                                                egui::FontId::proportional(48.0),
                                                theme::ACCENT,
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
