//! Render the in-game `M`-key world map (hypsometric relief + river ink) to a
//! PNG, so the river overlay can be tuned without launching the game.
//!
//! Run: cargo run --release --example world_map_preview
//! Out: target/river_proto/world_map.png

use image::RgbaImage;
use world_gen::world_core::config::GameConfig;
use world_gen::world_core::heightmap::Heightmap;
use world_gen::world_core::rivers::RiverField;
use world_gen::world_runtime::{render_world_map, WORLD_MAP_RES};

fn main() {
    std::fs::create_dir_all("target/river_proto").unwrap();
    let config = GameConfig::default();
    let seed = config.world.seed;
    let sea = config.sea_level;
    let hm = Heightmap::new(seed, config.heightmap.clone());
    let field = RiverField::generate(seed, &config);

    let res = WORLD_MAP_RES;
    let buf = render_world_map(&hm, sea, &field, res);
    let img = RgbaImage::from_raw(res as u32, res as u32, buf).unwrap();
    let out = "target/river_proto/world_map.png";
    img.save(out).unwrap();
    eprintln!("wrote {out} ({res}x{res})");
}
