//! Render the *actual* in-game global `RiverField` over the whole world, so we
//! can see where rivers form and pick coordinates to fly to in the engine.
//!
//! Run: cargo run --release --example river_field_map
//! Out: target/river_proto/world_rivers.png  (+ printed river coordinates)

use image::RgbImage;
use world_gen::world_core::chunk::WORLD_SIZE_METERS;
use world_gen::world_core::config::GameConfig;
use world_gen::world_core::heightmap::Heightmap;
use world_gen::world_core::rivers::RiverField;

const N: usize = 1024;

fn main() {
    std::fs::create_dir_all("target/river_proto").unwrap();
    let mut config = GameConfig::default();
    if let Ok(r) = std::env::var("RIVER_RES") {
        config.rivers.grid_resolution = r.parse().unwrap();
    }
    let seed = config.world.seed;
    let sea = config.sea_level;
    let hm = Heightmap::new(seed, config.heightmap.clone());
    let t0 = std::time::Instant::now();
    let field = RiverField::generate(seed, &config);
    eprintln!(
        "RiverField::generate(res={}) took {:.0} ms",
        config.rivers.grid_resolution,
        t0.elapsed().as_secs_f32() * 1000.0
    );

    let l = WORLD_SIZE_METERS as f32;
    let cell = l / N as f32;

    // Sample height + river field across the whole world.
    let mut h = vec![0.0f32; N * N];
    let mut wet = vec![0.0f32; N * N];
    for z in 0..N {
        for x in 0..N {
            let wx = x as f32 * cell;
            let wz = z as f32 * cell;
            h[z * N + x] = hm.sample_height(wx, wz);
            wet[z * N + x] = field.wetness(wx, wz);
        }
    }

    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| {
        [
            a[0] + (b[0] - a[0]) * t,
            a[1] + (b[1] - a[1]) * t,
            a[2] + (b[2] - a[2]) * t,
        ]
    };
    let mut img = RgbImage::new(N as u32, N as u32);
    for z in 0..N {
        for x in 0..N {
            let i = z * N + x;
            let hh = h[i];
            let mut c = if hh < sea {
                let d = ((sea - hh) / 80.0).clamp(0.0, 1.0);
                lerp([0.40, 0.62, 0.70], [0.04, 0.13, 0.32], d)
            } else {
                // Simple height shading + NW hillshade.
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(N - 1);
                let zm = z.saturating_sub(1);
                let zp = (z + 1).min(N - 1);
                let dzdx = (h[z * N + xp] - h[z * N + xm]) / (2.0 * cell);
                let dzdz = (h[zp * N + x] - h[zm * N + x]) / (2.0 * cell);
                let shade = (0.55 + 1.6 * (dzdx * 0.5 - dzdz * 0.5 + 0.2)).clamp(0.35, 1.2);
                let base = if hh < sea + 40.0 {
                    [0.30, 0.52, 0.24]
                } else if hh < 120.0 {
                    [0.20, 0.40, 0.18]
                } else if hh < 165.0 {
                    [0.46, 0.42, 0.38]
                } else {
                    [0.95, 0.96, 0.98]
                };
                [base[0] * shade, base[1] * shade, base[2] * shade]
            };
            if hh >= sea && wet[i] > 0.0 {
                c = lerp(c, [0.18, 0.40, 0.62], (0.4 + 0.6 * wet[i]).min(1.0));
            }
            img.put_pixel(
                x as u32,
                z as u32,
                image::Rgb([
                    (c[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (c[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (c[2].clamp(0.0, 1.0) * 255.0) as u8,
                ]),
            );
        }
    }
    let path = "target/river_proto/world_rivers.png";
    img.save(path).unwrap();
    println!("wrote {path}");

    // Print the strongest river cells (well-separated) as fly-to coordinates.
    let mut cells: Vec<(f32, usize)> = (0..N * N)
        .filter(|&i| h[i] >= sea && wet[i] > 0.25)
        .map(|i| (wet[i], i))
        .collect();
    cells.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut picked: Vec<(f32, f32)> = Vec::new();
    for (w, i) in cells {
        let wx = (i % N) as f32 * cell;
        let wz = (i / N) as f32 * cell;
        if picked
            .iter()
            .all(|&(px, pz)| (px - wx).hypot(pz - wz) > 4000.0)
        {
            println!("river @ world ({wx:.0}, {wz:.0})  wetness {w:.2}");
            picked.push((wx, wz));
            if picked.len() >= 8 {
                break;
            }
        }
    }
}
