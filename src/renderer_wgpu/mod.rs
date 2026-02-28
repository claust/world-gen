#[cfg(not(target_arch = "wasm32"))]
pub mod asset_watcher;
pub mod camera;
#[cfg(not(target_arch = "wasm32"))]
pub mod egui_bridge;
#[cfg(not(target_arch = "wasm32"))]
pub mod egui_pass;
pub mod geometry;
pub mod gpu_context;
pub mod instancing;
pub mod material;
#[cfg(not(target_arch = "wasm32"))]
pub mod model_loader;
pub mod pipeline;
pub mod sky;
pub mod terrain_compute;
pub mod world;

mod hud_font;
mod hud_pass;
mod instanced_pass;
mod terrain_pass;
mod water_pass;
