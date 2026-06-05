use std::f64::consts::TAU;

use noise::{NoiseFn, OpenSimplex};

use crate::world_core::chunk::WORLD_SIZE_METERS;
use crate::world_core::config::HeightmapConfig;

pub struct Heightmap {
    continental: OpenSimplex,
    ridge: OpenSimplex,
    detail: OpenSimplex,
    moisture: OpenSimplex,
    config: HeightmapConfig,
}

/// A spawn location picked by scanning the height field: a world position on
/// land right next to the coast, plus the direction toward the nearby sea so
/// the camera can be turned to look out over the water.
pub struct CoastalSpawn {
    /// World X of the spawn point (on land).
    pub x: f32,
    /// World Z of the spawn point (on land).
    pub z: f32,
    /// Terrain height at `(x, z)`; the camera sits a few metres above this.
    pub ground: f32,
    /// Unit direction in world XZ pointing toward the adjacent water.
    pub water_dir: (f32, f32),
}

impl Heightmap {
    pub fn new(seed: u32, config: HeightmapConfig) -> Self {
        Self {
            continental: OpenSimplex::new(seed),
            ridge: OpenSimplex::new(seed.wrapping_add(101)),
            detail: OpenSimplex::new(seed.wrapping_add(907)),
            moisture: OpenSimplex::new(seed.wrapping_add(1701)),
            config,
        }
    }

    /// Sample a noise layer so the field is exactly periodic with period
    /// `L = WORLD_SIZE_METERS` on both axes, matching seamlessly across the
    /// world-wrap seam.
    ///
    /// Each world axis is mapped onto a circle and the four circle coordinates
    /// are fed to 4D OpenSimplex (a torus embedded in 4D). For requested spatial
    /// frequency `f`, the loop must close after a whole number of noise cycles,
    /// so we snap the cycle count `k = round(f * L)` to an integer (the effective
    /// frequency becomes `k / L`, a hair off `f` — negligible at these `k`). The
    /// frequency the rest of the code reasons about stays continuous.
    ///
    /// `offset_x` / `offset_z` are phase shifts expressed in the same scaled
    /// (cycles) units as the original `x * f + offset` domain offset, applied as
    /// an angular rotation so decorrelated layers stay decorrelated on the torus.
    fn torus_sample(
        noise: &OpenSimplex,
        x: f64,
        z: f64,
        frequency: f64,
        offset_x: f64,
        offset_z: f64,
    ) -> f64 {
        let l = WORLD_SIZE_METERS;
        // Whole noise cycles around the world per axis; never 0 (would flatten
        // the layer) — clamp to at least 1.
        let k = (frequency * l).round().max(1.0);
        let radius = k / TAU;
        // arc length along the loop = scaled coordinate; an offset in scaled
        // units rotates the angle by offset * TAU / k.
        let theta_x = TAU * x / l + offset_x * TAU / k;
        let theta_z = TAU * z / l + offset_z * TAU / k;
        noise.get([
            radius * theta_x.cos(),
            radius * theta_x.sin(),
            radius * theta_z.cos(),
            radius * theta_z.sin(),
        ])
    }

    pub fn sample_height(&self, x: f32, z: f32) -> f32 {
        let x = x as f64;
        let z = z as f64;
        let c = &self.config;

        let broad =
            Self::torus_sample(&self.continental, x, z, c.continental.frequency, 0.0, 0.0) as f32;
        let ridges =
            1.0 - (Self::torus_sample(&self.ridge, x, z, c.ridge.frequency, 0.0, 0.0).abs() as f32);
        let rough = Self::torus_sample(&self.detail, x, z, c.detail.frequency, 0.0, 0.0) as f32;

        broad * c.continental.amplitude + ridges * c.ridge.amplitude + rough * c.detail.amplitude
    }

    pub fn sample_moisture(&self, x: f32, z: f32) -> f32 {
        let x = x as f64;
        let z = z as f64;
        let c = &self.config;
        let base =
            Self::torus_sample(&self.moisture, x, z, c.moisture_base_frequency, 0.0, 0.0) as f32;
        let variation = Self::torus_sample(
            &self.moisture,
            x,
            z,
            c.moisture_variation_frequency,
            c.moisture_variation_offset_x,
            c.moisture_variation_offset_z,
        ) as f32;
        ((base * c.moisture_base_weight + variation * c.moisture_variation_weight) * 0.5 + 0.5)
            .clamp(0.0, 1.0)
    }

    /// Scan the world for a coastline and return a pleasant spawn on land beside
    /// it, so a fresh world drops the player at the shore rather than in a
    /// corner.
    ///
    /// The world is sampled one point per chunk centre — the same resolution the
    /// full-world map (M) classifies land/water at, so the chosen cell matches
    /// what the player sees on the map. A chunk is "land" when its centre height
    /// is at or above `sea_level`. We sweep outward from the world centre in
    /// square rings and stop at the first ring that contains a land chunk
    /// bordering water (4-neighbourhood, with world wrap), keeping the cell
    /// nearest the centre within that ring — so the common case touches only a
    /// handful of cells instead of the whole world. Chunk-centre heights are
    /// cached so a neighbour shared between cells is never sampled twice. Having
    /// picked the cell, we step from its centre toward the water and stop just
    /// shy of the waterline so the spawn sits right on the shore. Returns `None`
    /// only if the world has no coastline at all.
    pub fn find_coastal_spawn(&self, sea_level: f32) -> Option<CoastalSpawn> {
        use crate::world_core::chunk::{CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS};
        use std::collections::HashMap;

        let n = WORLD_SIZE_CHUNKS;
        let chunk_centre = |c: i32| (c as f32 + 0.5) * CHUNK_SIZE_METERS;
        // Ring origin: the chunk nearest the world centre.
        let cc = n / 2;

        const NEIGHBOURS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

        // best = (distance² to centre, chunk x, chunk z, water dir)
        let best: Option<(f32, i32, i32, (f32, f32))> = {
            // Cache one height sample per chunk centre (keyed by wrapped coord)
            // so neighbour checks never re-sample the expensive noise field.
            let mut height_cache: HashMap<(i32, i32), f32> = HashMap::new();
            let mut height = |cx: i32, cz: i32| -> f32 {
                let key = (cx.rem_euclid(n), cz.rem_euclid(n));
                *height_cache
                    .entry(key)
                    .or_insert_with(|| self.sample_height(chunk_centre(key.0), chunk_centre(key.1)))
            };

            let mut found = None;
            // Rings up to n/2 cover the whole (wrapping) world.
            for r in 0..=(n / 2) {
                let mut best_in_ring: Option<(f32, i32, i32, (f32, f32))> = None;
                // Offsets whose Chebyshev distance from the centre is exactly r.
                let mut ring: Vec<(i32, i32)> = Vec::new();
                if r == 0 {
                    ring.push((0, 0));
                } else {
                    for ox in -r..=r {
                        ring.push((ox, -r));
                        ring.push((ox, r));
                    }
                    for oz in (-r + 1)..=(r - 1) {
                        ring.push((-r, oz));
                        ring.push((r, oz));
                    }
                }

                for (ox, oz) in ring {
                    let cx = cc + ox;
                    let cz = cc + oz;
                    if height(cx, cz) < sea_level {
                        continue; // water cell — spawn must be on land
                    }
                    // Face the single deepest adjacent water chunk (a cardinal
                    // direction, so the walk below heads at real water rather
                    // than a diagonal land corner) for the most open ocean view.
                    let mut water_dir: Option<(f32, f32)> = None;
                    let mut deepest = f32::MAX;
                    for (dx, dz) in NEIGHBOURS {
                        let nh = height(cx + dx, cz + dz);
                        if nh < sea_level && nh < deepest {
                            deepest = nh;
                            water_dir = Some((dx as f32, dz as f32));
                        }
                    }
                    let Some(dir) = water_dir else {
                        continue;
                    };
                    let dist2 = (ox * ox + oz * oz) as f32;
                    if best_in_ring.as_ref().is_none_or(|(bd, ..)| dist2 < *bd) {
                        best_in_ring = Some((dist2, cx.rem_euclid(n), cz.rem_euclid(n), dir));
                    }
                }

                // Stop at the first ring that has a coastline. A cell one ring
                // further out could be a hair closer in Euclidean terms (a
                // straight edge beating this ring's diagonal corner), but that
                // sub-ring difference doesn't matter for a spawn — this keeps the
                // scan to the centremost handful of cells.
                if best_in_ring.is_some() {
                    found = best_in_ring;
                    break;
                }
            }
            found
        };

        let (_, cx, cz, (ux, uz)) = best?;

        // Walk from the chunk centre toward the water in small steps, keeping the
        // last point that is still dry land, so the spawn ends up right at the
        // waterline. The water neighbour's centre is one chunk away, so the
        // land/water crossing lies within a full chunk — walk that far in fine
        // steps so even a distant or gently sloped crossing is reached, and stop
        // at the last point above sea level rather than a fixed margin (a gentle
        // coast would otherwise dip below a margin while still on dry land).
        let (mut sx, mut sz) = (chunk_centre(cx), chunk_centre(cz));
        let step = 8.0;
        let max_steps = (CHUNK_SIZE_METERS / step) as i32;
        for i in 1..=max_steps {
            let px = chunk_centre(cx) + ux * step * i as f32;
            let pz = chunk_centre(cz) + uz * step * i as f32;
            if self.sample_height(px, pz) > sea_level {
                sx = px;
                sz = pz;
            } else {
                break;
            }
        }

        // The walk only ever keeps points on land, so this is a genuine
        // above-water terrain height (no clamping needed).
        let ground = self.sample_height(sx, sz);
        Some(CoastalSpawn {
            x: sx,
            z: sz,
            ground,
            water_dir: (ux, uz),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Heightmap;
    use crate::world_core::chunk::WORLD_SIZE_METERS;
    use crate::world_core::config::HeightmapConfig;

    /// The terrain field must be exactly periodic with period `L` on both axes,
    /// so the world meets itself seamlessly across the wrap seam.
    #[test]
    fn height_and_moisture_wrap_seamlessly() {
        let hm = Heightmap::new(42, HeightmapConfig::default());
        let l = WORLD_SIZE_METERS as f32;

        for &(x, z) in &[
            (0.0, 0.0),
            (123.5, -777.0),
            (40_000.0, 5_120.0),
            (-321.0, 9_999.0),
        ] {
            let h0 = hm.sample_height(x, z);
            let m0 = hm.sample_moisture(x, z);
            // Shifting either axis by a whole world lap must reproduce the field.
            for &(dx, dz) in &[(l, 0.0), (0.0, l), (l, l), (-l, 2.0 * l)] {
                assert!(
                    (hm.sample_height(x + dx, z + dz) - h0).abs() < 1e-3,
                    "height not periodic at ({x},{z}) + ({dx},{dz})"
                );
                assert!(
                    (hm.sample_moisture(x + dx, z + dz) - m0).abs() < 1e-3,
                    "moisture not periodic at ({x},{z}) + ({dx},{dz})"
                );
            }
        }
    }

    /// A fresh world should spawn the player on land that sits right next to the
    /// sea: the chosen point is above water, and the water it faces is close by.
    #[test]
    fn coastal_spawn_is_on_land_beside_water() {
        use crate::world_core::chunk::CHUNK_SIZE_METERS;

        let hm = Heightmap::new(42, HeightmapConfig::default());
        let sea_level = 40.0;
        let spawn = hm
            .find_coastal_spawn(sea_level)
            .expect("the default world has a coastline");

        // The spawn point itself is on land.
        assert!(
            hm.sample_height(spawn.x, spawn.z) >= sea_level,
            "spawn must be above sea level"
        );
        assert!(spawn.ground >= sea_level);

        // Water is nearby in the reported direction: stepping out along it within
        // about a chunk and a half drops below sea level.
        let (ux, uz) = spawn.water_dir;
        let reach = 1.5 * CHUNK_SIZE_METERS;
        let crosses_into_water = (1..=24).any(|i| {
            let d = i as f32 / 24.0 * reach;
            hm.sample_height(spawn.x + ux * d, spawn.z + uz * d) < sea_level
        });
        assert!(
            crosses_into_water,
            "the coast should be within ~{reach:.0} m in the water direction"
        );
    }
}
