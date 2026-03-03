# UI Accessibility Tree for Debug API

## Problem

The debug CLI can control gameplay (camera, day speed, screenshots) but cannot navigate menus or interact with UI elements. When the game launches, it starts on the **Start Menu** — there's no way to programmatically click "Start Game" or adjust config sliders. This blocks full automation workflows.

## Goal

Add browser-accessibility-tree-style introspection to the debug API so that any automation client (CLI, Claude, test harness) can:

1. **Query** the current UI state — get a flat list of all interactive elements with stable IDs
2. **Interact** — click buttons, set slider values, select dropdown options, toggle checkboxes

## Design

### Three debug commands

#### `ui_snapshot` — Query interactive UI elements

Returns a flat list of every interactive widget currently registered, plus the current screen name. The snapshot is wrapped in the standard `CommandAppliedEvent` envelope under the `data` field.

> **Note:** On the config panel, elements inside collapsible sections are always included regardless of collapsed state, enabling debug API clients to set values without needing to expand sections first.

**Request:**
```json
{ "id": "cli-...", "type": "ui_snapshot" }
```

**Response (CommandAppliedEvent):**
```json
{
  "id": "cli-...",
  "frame": 12345,
  "ok": true,
  "message": "ui snapshot: 3 elements on start_menu",
  "data": {
    "screen": "start_menu",
    "elements": [
      { "id": "btn-start-game",     "type": "button",   "label": "Start Game" },
      { "id": "btn-plant-editor",   "type": "button",   "label": "Plant Editor" },
      { "id": "btn-exit",           "type": "button",   "label": "Exit" }
    ]
  }
}
```

Slider elements include value and range:
```json
{ "id": "slider-sea-level", "type": "slider", "label": "Sea Level", "value": 40.0, "min": 0.0, "max": 50.0 }
```

Combo elements include value and options:
```json
{ "id": "combo-species", "type": "combo", "label": "Species", "value": "Oak", "options": ["Oak", "Birch", "Acacia"] }
```

**Element types:**

| Type         | Fields                                        | Interaction          |
|--------------|-----------------------------------------------|----------------------|
| `button`     | `id`, `label`                                 | `ui_click`           |
| `slider`     | `id`, `label`, `value`, `min`, `max`          | `ui_set_value`       |
| `int_slider` | `id`, `label`, `value`, `min`, `max`          | `ui_set_value`       |
| `checkbox`   | `id`, `label`, `value` (bool)                 | `ui_click` (toggle)  |
| `combo`      | `id`, `label`, `value`, `options`             | `ui_set_value`       |

#### `ui_click` — Click a button or toggle a checkbox

**Request:**
```json
{ "id": "cli-...", "type": "ui_click", "element_id": "btn-start-game" }
```

**Response (success):**
```json
{ "id": "cli-...", "frame": 12345, "ok": true, "message": "ui click queued: btn-start-game" }
```

**Response (element not found):**
```json
{ "id": "cli-...", "frame": 12345, "ok": false, "message": "ui click failed: element 'btn-foo' not found" }
```

#### `ui_set_value` — Set a slider or combo value

**Request:**
```json
{ "id": "cli-...", "type": "ui_set_value", "element_id": "slider-sea-level", "value": "25" }
```

Value is always a string — parsed according to element type:
- **slider**: parsed as `f32`/`f64`, clamped to range
- **int_slider**: parsed as `i32`, clamped to range
- **combo**: matched against options (exact string match)
- **checkbox**: `"true"` / `"false"`

**Response (success):**
```json
{ "id": "cli-...", "frame": 12345, "ok": true, "message": "ui set_value queued: slider-sea-level = 25" }
```

**Response (element not found):**
```json
{ "id": "cli-...", "frame": 12345, "ok": false, "message": "ui set_value failed: element 'slider-foo' not found" }
```

### CLI interface

```bash
# Get all interactive elements
bun tools/debug-cli/cli.ts ui_snapshot

# Click a button
bun tools/debug-cli/cli.ts ui_click --element btn-start-game

# Set a slider value
bun tools/debug-cli/cli.ts ui_set_value --element slider-sea-level --value 25

# Set a combo selection
bun tools/debug-cli/cli.ts ui_set_value --element combo-species --value Birch
```

## Implementation

### Core idea: UI descriptor registry

Since egui is immediate-mode (no persistent widget tree), each UI panel **emits a descriptor list** alongside rendering. A shared registry collects these descriptors each frame so the debug API can serve them.

### Data structures

```rust
pub enum UiElementKind {
    Button,
    Slider { value: f64, min: f64, max: f64 },
    IntSlider { value: i64, min: i64, max: i64 },
    Checkbox { value: bool },
    Combo { value: String, options: Vec<String> },
}

pub struct UiElement {
    pub id: String,        // e.g. "btn-start-game", "slider-sea-level"
    pub label: String,     // Display text
    pub kind: UiElementKind,
}

pub struct UiSnapshot {
    pub screen: String,    // "start_menu", "playing", "plant_editor"
    pub elements: Vec<UiElement>,
}

pub enum UiAction {
    Click { element_id: String },
    SetValue { element_id: String, value: String },
}
```

### Registry (`UiRegistry`)

Lives in `AppState`, passed to UI panels during render:

```rust
pub struct UiRegistry {
    elements: Vec<UiElement>,
    pending_actions: Vec<UiAction>,
}

impl UiRegistry {
    pub fn register_button(&mut self, id: &str, label: &str);
    pub fn register_slider(&mut self, id: &str, label: &str, value: f64, min: f64, max: f64);
    pub fn register_int_slider(&mut self, id: &str, label: &str, value: i64, min: i64, max: i64);
    pub fn register_checkbox(&mut self, id: &str, label: &str, value: bool);
    pub fn register_combo(&mut self, id: &str, label: &str, value: &str, options: &[&str]);
    pub fn consume_click(&mut self, id: &str) -> bool;
    pub fn consume_set_value(&mut self, id: &str) -> Option<String>;
    pub fn has_element(&self, id: &str) -> bool;
    pub fn push_action(&mut self, action: UiAction);
    pub fn take_snapshot(&self, screen: &str) -> UiSnapshot;
    pub fn clear(&mut self);
}
```

### Per-frame flow

```
1. Frame starts
2. registry.clear()
3. UI panels render via egui AND register elements:
     start_menu.ui(ctx, &mut registry)
     config_panel.ui(ctx, &mut registry)   // registers ALL elements, even in collapsed sections
     plant_editor_panel.ui(ctx, &mut registry)
4. Each panel checks consume_click / consume_set_value for pending debug actions
5. Debug API serves snapshot on ui_snapshot request (reads from previous frame's elements)
6. Debug API validates element_id for ui_click/ui_set_value before queuing actions
```

### Panel instrumentation

Each panel's `ui()` method gets a `&mut UiRegistry` parameter. Registration happens alongside the existing egui calls:

```rust
// In start_menu.rs:
registry.register_button("btn-start-game", "Start Game");
if ui.button("Start Game").clicked() || registry.consume_click("btn-start-game") {
    return Some(MenuAction::StartGame);
}
```

```rust
// In config_panel.rs — register and consume happen outside collapsing sections:
reg_f32(registry, "slider-sea-level", "Sea Level", &mut self.config.sea_level, 0.0, 50.0);
// Visual widget inside collapsing body:
ui.add(egui::Slider::new(&mut self.config.sea_level, 0.0..=50.0).text("sea level"));
```

### ID scheme

Stable, human-readable IDs derived from element type + label:

| Pattern | Example |
|---------|---------|
| `btn-{kebab-label}` | `btn-start-game`, `btn-randomize` |
| `slider-{kebab-label}` | `slider-sea-level`, `slider-crown-base` |
| `section-{kebab-label}` | `section-heightmap`, `section-world` |
| `combo-{kebab-label}` | `combo-species`, `combo-crown-shape` |

### Config panel: collapsible sections

The config panel registers **section toggle buttons** (`section-heightmap`, `section-biomes`, etc.) that can be clicked via the debug API to expand/collapse sections. All slider elements are registered and actionable regardless of collapse state, enabling full automation without manual section expansion.

### Dirty config debouncing

Both `ConfigPanel::take_dirty_config` and `PlantEditorPanel::take_dirty_params` debounce changes by waiting for pointer release (to avoid regeneration mid-drag). When no pointer is active (debug API changes), they apply immediately.

## File changes

| File | Change |
|------|--------|
| `src/ui/ui_registry.rs` | New: `UiRegistry`, `UiElement`, `UiAction` types |
| `src/ui/mod.rs` | Export new types |
| `src/ui/start_menu.rs` | Register buttons, handle `consume_click` |
| `src/ui/config_panel.rs` | Register all sliders outside collapsing sections, section toggles |
| `src/ui/plant_editor_panel.rs` | Register combos, sliders, buttons |
| `src/debug_api/types.rs` | Add `UiSnapshot`, `UiClick`, `UiSetValue` command variants |
| `src/app/debug_commands.rs` | Handle new commands with element ID validation |
| `src/app/mod.rs` | Wire `UiRegistry` into render loop, menu/editor command handling |
| `tools/debug-cli/cli.ts` | Add `ui_snapshot`, `ui_click`, `ui_set_value` commands |

## Out of scope (for now)

- **Nested/hierarchical tree** — flat list is sufficient for the current UI complexity.
- **Text input fields** — no free-text inputs exist in the current UI. Add `ui_type` command if text fields are added later.
- **Drag interactions** — slider values are set directly, no need to simulate mouse drag.
- **Visual coordinates** — no pixel positions needed; all interaction is by element ID.
