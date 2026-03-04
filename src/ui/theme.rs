use egui::Color32;

// ── Background fills ────────────────────────────────────────────────────────

/// Dark green panel background used by herbarium, plant editor, etc.
/// Original unmultiplied: (10, 40, 15, 200)
pub const PANEL_BG: Color32 = Color32::from_rgba_premultiplied(8, 31, 12, 200);

/// Dark overlay for the start menu / title screen.
pub const OVERLAY_BG: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 120);

/// Dark green button fill (normal state).
/// Original unmultiplied: (30, 80, 30, 180)
pub const BUTTON_BG: Color32 = Color32::from_rgba_premultiplied(21, 56, 21, 180);

/// Dark green tile fill (normal state).
/// Original unmultiplied: (30, 70, 30, 180)
pub const TILE_BG: Color32 = Color32::from_rgba_premultiplied(21, 49, 21, 180);

/// Dark green tile fill (hovered).
/// Original unmultiplied: (50, 100, 50, 200)
pub const TILE_BG_HOVER: Color32 = Color32::from_rgba_premultiplied(39, 78, 39, 200);

// ── Danger / destructive ────────────────────────────────────────────────────

/// Red button fill for destructive actions.
pub const DANGER_BG: Color32 = Color32::from_rgb(80, 20, 20);

/// Light red text for destructive actions.
pub const DANGER_TEXT: Color32 = Color32::from_rgb(255, 120, 120);

// ── Accent & text ───────────────────────────────────────────────────────────

/// Primary accent: light green used for titles, borders, highlights.
pub const ACCENT: Color32 = Color32::from_rgb(140, 220, 120);

/// Slightly muted green accent (e.g. tile borders on new-plant tile).
pub const ACCENT_MUTED: Color32 = Color32::from_rgb(100, 180, 90);

/// Soft grey for secondary labels (premultiplied from white at 63% opacity).
pub const TEXT_SECONDARY: Color32 = Color32::from_rgba_premultiplied(160, 160, 160, 160);

// ── Layout constants ────────────────────────────────────────────────────────

/// Standard button text size.
pub const BUTTON_TEXT_SIZE: f32 = 16.0;

/// Title text size for full-screen views (herbarium, start menu).
pub const TITLE_SIZE: f32 = 42.0;

/// Title text size for side panels (plant editor, config).
pub const PANEL_TITLE_SIZE: f32 = 28.0;

/// Section header text size (collapsing headers).
pub const SECTION_HEADER_SIZE: f32 = 15.0;

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Lighten a color by adding `amount` to each RGB channel.
pub fn lighten(c: Color32, amount: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(
        c.r().saturating_add(amount),
        c.g().saturating_add(amount),
        c.b().saturating_add(amount),
        c.a(),
    )
}

/// Convert HSL to Color32 (used for plant-derived tile colors).
pub fn hsl_to_color32(h: f32, s: f32, l: f32, alpha: u8) -> Color32 {
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

/// Create a styled menu button (larger text, for start screen / main navigation).
pub fn menu_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(label).size(20.0).color(Color32::WHITE))
        .fill(BUTTON_BG)
        .stroke(egui::Stroke::new(1.0, ACCENT_MUTED))
}

/// Create a styled danger (red) button for destructive actions.
pub fn danger_button(label: &str) -> egui::Button<'_> {
    egui::Button::new(
        egui::RichText::new(label)
            .size(BUTTON_TEXT_SIZE)
            .color(DANGER_TEXT),
    )
    .fill(DANGER_BG)
    .stroke(egui::Stroke::new(1.0, DANGER_TEXT))
}

/// Create a styled title label.
pub fn title(text: &str, size: f32) -> egui::RichText {
    egui::RichText::new(text).size(size).color(ACCENT)
}

/// Create a styled section header (for collapsing headers).
pub fn section_header(text: &str) -> egui::RichText {
    egui::RichText::new(text)
        .size(SECTION_HEADER_SIZE)
        .color(ACCENT)
}

/// Back button size (square, touch-friendly).
pub const BACK_BUTTON_SIZE: f32 = 44.0;

/// Create a large square back button with a chevron, suitable for touch.
pub fn back_button() -> egui::Button<'static> {
    egui::Button::new(egui::RichText::new("<").size(28.0).color(Color32::WHITE))
        .fill(BUTTON_BG)
        .stroke(egui::Stroke::new(1.0, ACCENT_MUTED))
}
