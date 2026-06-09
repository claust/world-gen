//! One-off probe for the river water-overflow bug: samples the river field on
//! a fine grid around a known overflow spot and reports every vertex where the
//! water sheet would render above terrain that was never carved (the
//! translucent sheet lying on the bank).
//!
//! Run:
//!   cargo run --release --example river_overflow_probe

use world_gen::world_core::chunk::RIVER_SURFACE_THRESHOLD;
use world_gen::world_core::config::GameConfig;
use world_gen::world_core::heightmap::Heightmap;
use world_gen::world_core::rivers::RiverField;

fn main() {
    let seed = 42u32;
    let config = GameConfig::default();
    println!("river grid resolution: {}", config.rivers.grid_resolution);
    let field = RiverField::generate(seed, &config);
    let hm = Heightmap::new(seed, config.heightmap.clone());

    // Center of the observed overflow (user's saved favorite spot). Override
    // with args: river_overflow_probe <cx> <cz> [half] [step]
    let argv: Vec<f32> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let cx = argv.first().copied().unwrap_or(30360.0f32);
    let cz = argv.get(1).copied().unwrap_or(24680.0f32);
    let half = argv.get(2).copied().unwrap_or(80.0f32);
    let step = argv.get(3).copied().unwrap_or(2.0f32);

    let mut channel = 0usize; // water over a properly carved bed (>= 1 m cut)
    let mut overflow = 0usize; // water over near-natural ground (< 1 m cut)
    let mut worst: Option<(f32, f32, f32, f32, f32, f32, f32)> = None;
    let n = (2.0 * half / step) as i32;
    let mut map = String::new();
    for iz in 0..=n {
        for ix in 0..=n {
            let x = cx - half + ix as f32 * step;
            let z = cz - half + iz as f32 * step;
            let raw = hm.sample_height(x, z);
            let (h, wet, sheet) = field.sample(raw, x, z);
            let depth = sheet - h;
            let carve = raw - h;
            if wet >= RIVER_SURFACE_THRESHOLD && depth > 0.0 {
                if carve >= 1.0 {
                    channel += 1;
                    map.push('#');
                } else {
                    overflow += 1;
                    map.push('~');
                    if worst.is_none() || depth > worst.unwrap().4 {
                        worst = Some((x, z, raw, sheet, depth, wet, carve));
                    }
                }
            } else {
                map.push(if wet >= RIVER_SURFACE_THRESHOLD {
                    '.'
                } else {
                    ' '
                });
            }
        }
        map.push('\n');
    }
    println!("sampled {}x{} points around ({cx},{cz})", n + 1, n + 1);
    println!("channel water (carve >= 1m): {channel}");
    println!("OVERFLOW water (carve < 1m, sheet above near-natural ground): {overflow}");
    println!("\nmap (# channel, ~ overflow, . wet-but-dry, blank dry):\n{map}");
    if let Some((x, z, raw, sheet, depth, wet, carve)) = worst {
        println!(
            "worst overflow: ({x},{z}) raw {raw:.2} sheet {sheet:.2} depth {depth:.2} wet {wet:.3} carve {carve:.2}"
        );
        println!("\nprofile along x at z={z}:");
        for i in -25..=25 {
            let px = x + i as f32 * 2.0;
            let raw = hm.sample_height(px, z);
            let (h, wet, sheet) = field.sample(raw, px, z);
            let mark = if wet >= RIVER_SURFACE_THRESHOLD && sheet > h {
                if raw - h < 1.0 {
                    "OVERFLOW"
                } else {
                    "water"
                }
            } else {
                ""
            };
            println!(
                "  x={px:8.1} raw={raw:7.2} carved={h:7.2} sheet={sheet:7.2} wet={wet:5.3} {mark}"
            );
        }
    }
}
