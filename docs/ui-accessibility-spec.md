# UI Accessibility Tree for Debug API

## Problem

The debug CLI can control gameplay (camera, day speed, screenshots) but cannot navigate menus or interact with UI elements. When the game launches, it starts on the **Start Menu** — there's no way to programmatically click "Start Game" or adjust config sliders. This blocks full automation workflows.

## Goal

Add browser-accessibility-tree-style introspection to the debug API so that any automation client (CLI, Claude, test harness) can:

1. **Query** the current UI state — get a flat list of all visible, interactive elements with stable IDs
2. **Interact** — click buttons, set slider values, select dropdown options, toggle checkboxes

## Design

### Two new commands

#### `ui_snapshot` — Query visible UI elements

Returns a flat list of every interactive widget currently on screen, plus the current screen name for context.

**Request:**
```json
{ "id": "cli-...", "type": "ui_snapshot" }
```

**Response:**
```json
{
  "screen": "start_menu",
  "elements": [
    { "id": "btn-start-game",     "type": "button",   "label": "Start Game" },
    { "id": "btn-plant-editor",   "type": "button",   "label": "Plant Editor" },
    { "id": "btn-exit",           "type": "button",   "label": "Exit" }
  ]
}
```

A gameplay example with config panel open:
```json
{
  "screen": "playing",
  "elements": [
    { "id": "slider-base-height",    "type": "slider", "label": "Base Height",    "value": 40.0, "range": [0.0, 200.0] },
    { "id": "slider-noise-scale",    "type": "slider", "label": "Noise Scale",    "value": 0.005, "range": [0.001, 0.1] },
    { "id": "checkbox-show-ferns",   "type": "checkbox", "label": "Show Ferns",   "value": true },
    { "id": "combo-biome-type",      "type": "combo",  "label": "Biome Type",     "value": "Temperate", "options": ["Temperate", "Desert", "Tundra"] }
  ]
}
```

Plant editor example:
```json
{
  "screen": "plant_editor",
  "elements": [
    { "id": "combo-species",          "type": "combo",   "label": "Species",        "value": "Oak", "options": ["Oak", "Birch", "Acacia", "Palm", "Shrub", "Spruce", "Willow"] },
    { "id": "btn-randomize",          "type": "button",  "label": "Randomize" },
    { "id": "slider-crown-radius",    "type": "slider",  "label": "Crown Radius",   "value": 3.5, "range": [0.5, 15.0] },
    { "id": "slider-trunk-height",    "type": "slider",  "label": "Trunk Height",   "value": 5.0, "range": [0.5, 30.0] }
  ]
}
```

**Element types:**

| Type       | Fields                                      | Interaction          |
|------------|---------------------------------------------|----------------------|
| `button`   | `id`, `label`                               | `ui_click`           |
| `slider`   | `id`, `label`, `value`, `range`             | `ui_set_value`       |
| `checkbox` | `id`, `label`, `value` (bool)               | `ui_click` (toggle)  |
| `combo`    | `id`, `label`, `value`, `options`           | `ui_set_value`       |

#### `ui_click` — Click a button or toggle a checkbox

**Request:**
```json
{ "id": "cli-...", "type": "ui_click", "element_id": "btn-start-game" }
```

**Response:**
```json
{ "status": "ok" }
```

If the element doesn't exist or isn't clickable:
```json
{ "status": "error", "message": "Element 'btn-foo' not found" }
```

#### `ui_set_value` — Set a slider or combo value

**Request:**
```json
{ "id": "cli-...", "type": "ui_set_value", "element_id": "slider-base-height", "value": "80.0" }
```

Value is always a string — parsed according to element type:
- **slider**: parsed as `f64`, clamped to range
- **combo**: matched against options (exact string match)
- **checkbox**: `"true"` / `"false"`

**Response:**
```json
{ "status": "ok", "value": "80.0" }
```

Returns the actual value after clamping/validation.

### CLI interface

```bash
# Get all interactive elements
bun tools/debug-cli/cli.ts ui_snapshot

# Click a button
bun tools/debug-cli/cli.ts ui_click --element btn-start-game

# Set a slider value
bun tools/debug-cli/cli.ts ui_set_value --element slider-base-height --value 80

# Set a combo selection
bun tools/debug-cli/cli.ts ui_set_value --element combo-species --value Birch
```

## Implementation approach

### Core idea: UI descriptor registry

Since egui is immediate-mode (no persistent widget tree), each UI panel **emits a descriptor list** alongside rendering. A shared registry collects these descriptors each frame so the debug API can serve them.

### Data structures

```rust
// Shared between UI panels and debug API
pub enum UiElementKind {
    Button,
    Slider { value: f64, min: f64, max: f64 },
    Checkbox { value: bool },
    Combo { value: String, options: Vec<String> },
}

pub struct UiElement {
    pub id: String,        // e.g. "btn-start-game", "slider-crown-radius"
    pub label: String,     // Display text
    pub kind: UiElementKind,
}

pub struct UiSnapshot {
    pub screen: String,    // "start_menu", "playing", "plant_editor"
    pub elements: Vec<UiElement>,
}

// Pending interaction from debug API
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
    pub fn register(&mut self, element: UiElement);
    pub fn take_snapshot(&mut self, screen: &str) -> UiSnapshot;
    pub fn push_action(&mut self, action: UiAction);
    pub fn drain_actions(&mut self) -> Vec<UiAction>;
}
```

### Per-frame flow

```
1. Frame starts
2. registry.clear()
3. UI panels render via egui AND register elements:
     start_menu.ui(ctx, &mut registry)
     config_panel.ui(ctx, &mut registry)
     plant_editor_panel.ui(ctx, &mut registry)
4. Registry checks pending_actions:
     - Button click → return MenuAction / trigger callback
     - Slider set → write value directly, mark config dirty
     - Combo set → update selection
5. Store latest UiSnapshot for debug API to serve
6. Debug API serves snapshot on ui_snapshot request
```

### Panel instrumentation

Each panel's `ui()` method gets a `&mut UiRegistry` parameter. Registration happens alongside the existing egui calls:

```rust
// In start_menu.rs — before:
if ui.button("Start Game").clicked() {
    return Some(MenuAction::StartGame);
}

// After:
registry.register_button("btn-start-game", "Start Game");
if ui.button("Start Game").clicked() || registry.consume_click("btn-start-game") {
    return Some(MenuAction::StartGame);
}
```

```rust
// In config_panel.rs — before:
ui.add(egui::Slider::new(&mut config.base_height, 0.0..=200.0).text("Base Height"));

// After:
registry.register_slider("slider-base-height", "Base Height", config.base_height, 0.0, 200.0);
if let Some(v) = registry.consume_set_value("slider-base-height") {
    config.base_height = v.parse().unwrap_or(config.base_height);
}
ui.add(egui::Slider::new(&mut config.base_height, 0.0..=200.0).text("Base Height"));
```

### ID scheme

Stable, human-readable IDs derived from element type + label:

| Pattern | Example |
|---------|---------|
| `btn-{kebab-label}` | `btn-start-game`, `btn-randomize` |
| `slider-{kebab-label}` | `slider-base-height`, `slider-crown-radius` |
| `checkbox-{kebab-label}` | `checkbox-show-ferns` |
| `combo-{kebab-label}` | `combo-species`, `combo-biome-type` |

Labels are lowercased and spaces replaced with hyphens. If duplicates occur (unlikely given the current UI), append `-2`, `-3`, etc.

### New command variants

Add to `CommandKind` enum in `src/debug_api/types.rs`:

```rust
#[serde(rename = "ui_snapshot")]
UiSnapshot,

#[serde(rename = "ui_click")]
UiClick { element_id: String },

#[serde(rename = "ui_set_value")]
UiSetValue { element_id: String, value: String },
```

### CLI additions

Add three new commands to `tools/debug-cli/cli.ts`:

- `ui_snapshot` — send command, print elements as formatted table
- `ui_click --element <id>` — send click command
- `ui_set_value --element <id> --value <val>` — send set-value command

## File changes

| File | Change |
|------|--------|
| `src/ui/mod.rs` | Add `UiRegistry`, `UiElement`, `UiAction` types |
| `src/ui/start_menu.rs` | Register buttons, handle `consume_click` |
| `src/ui/config_panel.rs` | Register sliders/checkboxes/combos, handle `consume_set_value` |
| `src/ui/plant_editor_panel.rs` | Register all 72+ params, species combo, randomize button |
| `src/debug_api/types.rs` | Add `UiSnapshot`, `UiClick`, `UiSetValue` command variants |
| `src/app/debug_commands.rs` | Handle new commands: snapshot from registry, push actions |
| `src/app/mod.rs` | Wire `UiRegistry` into render loop, store latest snapshot |
| `tools/debug-cli/cli.ts` | Add `ui_snapshot`, `ui_click`, `ui_set_value` commands |

## Out of scope (for now)

- **Nested/hierarchical tree** — flat list is sufficient for the current UI complexity. Collapsible section headers could be added as `section` elements later if needed.
- **Text input fields** — no free-text inputs exist in the current UI. Add `ui_type` command if text fields are added later.
- **Drag interactions** — slider values are set directly, no need to simulate mouse drag.
- **Visual coordinates** — no pixel positions needed; all interaction is by element ID.
