use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiElementKind {
    Button,
    Slider { value: f64, min: f64, max: f64 },
    Checkbox { value: bool },
    Combo { value: String, options: Vec<String> },
    IntSlider { value: i64, min: i64, max: i64 },
}

#[derive(Debug, Clone, Serialize)]
pub struct UiElement {
    pub id: String,
    pub label: String,
    #[serde(flatten)]
    pub kind: UiElementKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct UiSnapshot {
    pub screen: String,
    pub elements: Vec<UiElement>,
}

#[derive(Debug, Clone)]
pub enum UiAction {
    Click { element_id: String },
    SetValue { element_id: String, value: String },
}

pub struct UiRegistry {
    elements: Vec<UiElement>,
    pending_actions: Vec<UiAction>,
}

impl Default for UiRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl UiRegistry {
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
            pending_actions: Vec::new(),
        }
    }

    pub fn clear(&mut self) {
        self.elements.clear();
    }

    pub fn register(&mut self, element: UiElement) {
        self.elements.push(element);
    }

    pub fn register_button(&mut self, id: &str, label: &str) {
        self.register(UiElement {
            id: id.to_string(),
            label: label.to_string(),
            kind: UiElementKind::Button,
        });
    }

    pub fn register_slider(&mut self, id: &str, label: &str, value: f64, min: f64, max: f64) {
        self.register(UiElement {
            id: id.to_string(),
            label: label.to_string(),
            kind: UiElementKind::Slider { value, min, max },
        });
    }

    pub fn register_int_slider(&mut self, id: &str, label: &str, value: i64, min: i64, max: i64) {
        self.register(UiElement {
            id: id.to_string(),
            label: label.to_string(),
            kind: UiElementKind::IntSlider { value, min, max },
        });
    }

    pub fn register_checkbox(&mut self, id: &str, label: &str, value: bool) {
        self.register(UiElement {
            id: id.to_string(),
            label: label.to_string(),
            kind: UiElementKind::Checkbox { value },
        });
    }

    pub fn register_combo(&mut self, id: &str, label: &str, value: &str, options: &[&str]) {
        self.register(UiElement {
            id: id.to_string(),
            label: label.to_string(),
            kind: UiElementKind::Combo {
                value: value.to_string(),
                options: options.iter().map(|s| s.to_string()).collect(),
            },
        });
    }

    /// Check if a click action is pending for this element ID and consume it.
    pub fn consume_click(&mut self, id: &str) -> bool {
        if let Some(pos) = self
            .pending_actions
            .iter()
            .position(|a| matches!(a, UiAction::Click { element_id } if element_id == id))
        {
            self.pending_actions.remove(pos);
            true
        } else {
            false
        }
    }

    /// Check if a set-value action is pending for this element ID and consume it.
    pub fn consume_set_value(&mut self, id: &str) -> Option<String> {
        if let Some(pos) = self
            .pending_actions
            .iter()
            .position(|a| matches!(a, UiAction::SetValue { element_id, .. } if element_id == id))
        {
            match self.pending_actions.remove(pos) {
                UiAction::SetValue { value, .. } => Some(value),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn push_action(&mut self, action: UiAction) {
        self.pending_actions.push(action);
    }

    pub fn take_snapshot(&self, screen: &str) -> UiSnapshot {
        UiSnapshot {
            screen: screen.to_string(),
            elements: self.elements.clone(),
        }
    }
}
