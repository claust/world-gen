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
    /// is at or above `sea_level`. Among all land chunks that border a water
    /// chunk (4-neighbourhood, with world wrap), the one nearest the world
    /// centre is chosen; then we step from that chunk's centre toward the water
    /// and stop just shy of the waterline so the spawn sits right on the shore.
    /// Returns `None` only if the world has no coastline at all.
    pub fn find_coastal_spawn(&self, sea_level: f32) -> Option<CoastalSpawn> {
        use crate::world_core::chunk::{CHUNK_SIZE_METERS, WORLD_SIZE_CHUNKS};

        let n = WORLD_SIZE_CHUNKS;
        let centre_chunk = (n as f32 - 1.0) * 0.5;
        let chunk_centre = |c: i32| (c as f32 + 0.5) * CHUNK_SIZE_METERS;

        // Land/water at every chunk centre, computed once so the coastline scan
        // is a cheap array lookup instead of re-sampling per neighbour.
        let mut land = vec![false; (n * n) as usize];
        for cz in 0..n {
            for cx in 0..n {
                let h = self.sample_height(chunk_centre(cx), chunk_centre(cz));
                land[(cz * n + cx) as usize] = h >= sea_level;
            }
        }
        let is_land = |cx: i32, cz: i32| land[(cz * n + cx) as usize];

        const NEIGHBOURS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];
        // best = (distance² to world centre, chunk x, chunk z, water dir)
        let mut best: Option<(f32, i32, i32, (f32, f32))> = None;
        for cz in 0..n {
            for cx in 0..n {
                if !is_land(cx, cz) {
                    continue;
                }
                // Face the single deepest adjacent water chunk (a cardinal
                // direction, so the walk below heads at real water rather than a
                // diagonal land corner) for the most open ocean view.
                let mut water_dir: Option<(f32, f32)> = None;
                let mut deepest = f32::MAX;
                for (dx, dz) in NEIGHBOURS {
                    let nx = (cx + dx).rem_euclid(n);
                    let nz = (cz + dz).rem_euclid(n);
                    if is_land(nx, nz) {
                        continue;
                    }
                    let h = self.sample_height(chunk_centre(nx), chunk_centre(nz));
                    if h < deepest {
                        deepest = h;
                        water_dir = Some((dx as f32, dz as f32));
                    }
                }
                let Some(dir) = water_dir else {
                    continue;
                };
                let ddx = cx as f32 - centre_chunk;
                let ddz = cz as f32 - centre_chunk;
                let dist2 = ddx * ddx + ddz * ddz;
                if best.as_ref().is_none_or(|(bd, ..)| dist2 < *bd) {
                    best = Some((dist2, cx, cz, dir));
                }
            }
        }

        let (_, cx, cz, (ux, uz)) = best?;

        // Walk from the chunk centre toward the water, keeping the last point
        // that is still comfortably on land, so the spawn ends up by the shore.
        let (mut sx, mut sz) = (chunk_centre(cx), chunk_centre(cz));
        let step = 16.0;
        for i in 1..=8 {
            let px = chunk_centre(cx) + ux * step * i as f32;
            let pz = chunk_centre(cz) + uz * step * i as f32;
            if self.sample_height(px, pz) > sea_level + 2.0 {
                sx = px;
                sz = pz;
            } else {
                break;
            }
        }

        let ground = self.sample_height(sx, sz).max(sea_level);
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
