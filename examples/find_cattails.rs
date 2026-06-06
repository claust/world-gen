//! Locate cattail (aquatic reed) beds and measure their share of the plant
//! population. Generates a block of chunks through the real base-world pipeline,
//! tallies plants per species, and prints the densest cattail chunks with a
//! world-space position to teleport the camera to for visual verification.
//!
//! Run:
//!   cargo run --release --example find_cattails
//!   FIND_RADIUS=30 FIND_CX=123 FIND_CZ=120 cargo run --release --example find_cattails

use std::sync::Arc;

use glam::IVec2;

use world_gen::world_core::chunk::CHUNK_SIZE_METERS;
use world_gen::world_core::chunk_generator::ChunkGenerator;
use world_gen::world_core::config::GameConfig;
use world_gen::world_core::herbarium::{Herbarium, PlantRegistry};
use world_gen::world_core::rivers::RiverField;

fn env_i32(key: &str, default: i32) -> i32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    let cx = env_i32("FIND_CX", 123);
    let cz = env_i32("FIND_CZ", 120);
    let radius = env_i32("FIND_RADIUS", 30);

    let config = GameConfig::default();
    let seed = config.world.seed;
    let herb = Herbarium::default_seeded();
    let registry = Arc::new(PlantRegistry::from_herbarium(&herb));

    let cattail_idx = registry
        .species
        .iter()
        .position(|s| s.name == "Cattail")
        .expect("Cattail species should be registered");
    println!("Cattail species_index = {cattail_idx}");
    println!(
        "Scanning {0}×{0} chunks around ({cx}, {cz}) (sea_level = {1})\n",
        radius * 2 + 1,
        config.sea_level
    );

    let rivers = Arc::new(RiverField::generate(seed, &config));
    let generator = ChunkGenerator::new(seed, &config, Arc::clone(&registry), Arc::clone(&rivers));

    let n_species = registry.species.len();
    let mut totals = vec![0u64; n_species];
    let mut total_plants = 0u64;
    // (cattail_count, chunk, a sample cattail world position)
    let mut best: Vec<(usize, IVec2, [f32; 3])> = Vec::new();

    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let coord = IVec2::new(cx + dx, cz + dz);
            let data = generator.generate_chunk(coord);
            let plants = &data.content.base_plants;
            total_plants += plants.len() as u64;

            let mut cattails_here = 0usize;
            let mut sample = [0.0f32; 3];
            for p in plants {
                if p.species_index < n_species {
                    totals[p.species_index] += 1;
                }
                if p.species_index == cattail_idx {
                    cattails_here += 1;
                    sample = [p.position.x, p.position.y, p.position.z];
                }
            }
            if cattails_here > 0 {
                best.push((cattails_here, coord, sample));
            }
        }
    }

    best.sort_by(|a, b| b.0.cmp(&a.0));

    println!("--- per-species totals in scanned region ---");
    let mut pairs: Vec<(usize, u64)> = totals.iter().copied().enumerate().collect();
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
    for (i, count) in pairs {
        if count == 0 {
            continue;
        }
        let pct = 100.0 * count as f64 / total_plants as f64;
        let marker = if i == cattail_idx {
            "  <== cattail"
        } else {
            ""
        };
        println!(
            "  {:<10} {:>10} ({:>5.2}%){}",
            registry.species[i].name, count, pct, marker
        );
    }
    println!("  {:<10} {:>10}", "TOTAL", total_plants);

    let cattail_total: u64 = totals[cattail_idx];
    println!(
        "\nCattails: {cattail_total} of {total_plants} plants ({:.3}%) across {} chunks with cattails",
        100.0 * cattail_total as f64 / total_plants.max(1) as f64,
        best.len()
    );

    println!("\n--- densest cattail chunks (teleport targets) ---");
    for (count, coord, sample) in best.iter().take(8) {
        let chunk_world_x = coord.x as f32 * CHUNK_SIZE_METERS;
        let chunk_world_z = coord.y as f32 * CHUNK_SIZE_METERS;
        println!(
            "  chunk ({:>4},{:>4})  {:>3} cattails  sample plant at ({:.0}, {:.1}, {:.0})  chunk-origin ({:.0}, {:.0})",
            coord.x, coord.y, count, sample[0], sample[1], sample[2], chunk_world_x, chunk_world_z
        );
    }
}
