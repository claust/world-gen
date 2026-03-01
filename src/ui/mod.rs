mod config_panel;
#[cfg(not(target_arch = "wasm32"))]
mod start_menu;

pub use config_panel::ConfigPanel;
#[cfg(not(target_arch = "wasm32"))]
pub use start_menu::{MenuAction, StartMenu};
