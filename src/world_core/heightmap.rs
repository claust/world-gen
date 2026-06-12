use std::f64::consts::TAU;
use std::sync::Arc;

use crate::world_core::chunk::WORLD_SIZE_METERS;
use crate::world_core::config::HeightmapConfig;
use crate::world_core::noise4::Simplex4;
use crate::world_core::terrain_fields::{FieldKind, TerrainFields};

pub struct Heightmap {
    /// Raw continental, ridge, and moisture (base + variation) noise, precomputed
    /// once on a coarse global grid and bilinearly sampled (these low-frequency
    /// octaves barely change within a chunk). The ridge crease, amplitudes, and
    /// moisture weights are still applied per vertex.
    fields: Arc<TerrainFields>,
    detail: Simplex4,
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
            fields: TerrainFields::shared(seed, &config),
            detail: Simplex4::new(seed.wrapping_add(907)),
            config,
        }
    }

    /// Sample a noise layer so the field is exactly periodic with period
    /// `L = WORLD_SIZE_METERS` on both axes, matching seamlessly across the
    /// world-wrap seam.
    ///
    /// Each world axis is mapped onto a circle and the four circle coordinates
    /// are fed to 4D noise (a torus embedded in 4D). For requested spatial
    /// frequency `f`, the loop must close after a whole number of noise cycles,
    /// so we snap the cycle count `k = round(f * L)` to an integer (the effective
    /// frequency becomes `k / L`, a hair off `f` — negligible at these `k`). The
    /// frequency the rest of the code reasons about stays continuous.
    ///
    /// `offset_x` / `offset_z` are phase shifts expressed in the same scaled
    /// (cycles) units as the original `x * f + offset` domain offset, applied as
    /// an angular rotation so decorrelated layers stay decorrelated on the torus.
    fn torus_sample(
        noise: &Simplex4,
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
        let c = &self.config;

        // Continental + ridge are bilinear lookups into the precomputed coarse
        // field; only the high-frequency detail octave is still a 4D noise eval.
        let (broad, ridge_raw) = self.fields.sample(x, z);
        let ridges = 1.0 - ridge_raw.abs(); // crease reconstructed at full res
        let rough = Self::torus_sample(
            &self.detail,
            x as f64,
            z as f64,
            c.detail.frequency,
            0.0,
            0.0,
        ) as f32;

        broad * c.continental.amplitude + ridges * c.ridge.amplitude + rough * c.detail.amplitude
    }

    pub fn sample_moisture(&self, x: f32, z: f32) -> f32 {
        let c = &self.config;
        // Both moisture octaves are bilinear lookups into the precomputed coarse
        // field; the weights and clamp are the only per-vertex work left.
        let (base, variation) = self.fields.sample_moisture(x, z);
        ((base * c.moisture_base_weight + variation * c.moisture_variation_weight) * 0.5 + 0.5)
            .clamp(0.0, 1.0)
    }

    /// Fill `out` with the per-axis torus trig for `out.len()` evenly spaced
    /// samples starting at world coordinate `origin` with `cell_size` spacing.
    ///
    /// `torus_sample` maps a world axis onto a circle and feeds the two circle
    /// coordinates (`radius·cosθ`, `radius·sinθ`) to 4D noise. Those two
    /// numbers depend only on that one axis, so for a regular grid the cost can
    /// be hoisted: the X pair is shared by every row, the Z pair by every column.
    /// This computes one axis's pairs once. The arithmetic is byte-for-byte the
    /// same as `torus_sample` (same `k`, `radius`, and `θ = TAU·w/L + offset·TAU/k`),
    /// so the grid samplers below reproduce the point samplers bit-exactly.
    fn fill_axis_trig(
        out: &mut [(f64, f64)],
        frequency: f64,
        offset: f64,
        origin: f32,
        cell_size: f32,
    ) {
        let l = WORLD_SIZE_METERS;
        let k = (frequency * l).round().max(1.0);
        let radius = k / TAU;
        for (i, slot) in out.iter_mut().enumerate() {
            let w = (origin + i as f32 * cell_size) as f64;
            let theta = TAU * w / l + offset * TAU / k;
            *slot = (radius * theta.cos(), radius * theta.sin());
        }
    }

    /// Fill a `side × side` field row by row, parallelising over rows — but only
    /// when not already running inside a rayon worker. The bulk base build and
    /// the streaming loader both parallelise at the chunk level, so a nested
    /// per-row `par_iter` there would just add fork/join overhead; a top-level
    /// caller (e.g. a single synchronous chunk) still gets the inner parallelism.
    fn fill_grid(side: usize, fill_row: impl Fn(usize, &mut [f32]) + Sync) -> Vec<f32> {
        let mut out = vec![0.0f32; side * side];
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            if rayon::current_thread_index().is_none() {
                out.par_chunks_mut(side)
                    .enumerate()
                    .for_each(|(zi, row)| fill_row(zi, row));
                return out;
            }
        }
        out.chunks_mut(side)
            .enumerate()
            .for_each(|(zi, row)| fill_row(zi, row));
        out
    }

    /// Height field for a `side × side` grid whose `(0,0)` vertex is at world
    /// `(origin_x, origin_z)` with `cell_size` spacing. Bit-identical to calling
    /// [`sample_height`](Self::sample_height) at each vertex, but hoists the
    /// per-axis torus trig out of the inner loop — the dominant cost of base-world
    /// generation.
    pub fn height_grid(
        &self,
        origin_x: f32,
        origin_z: f32,
        side: usize,
        cell_size: f32,
    ) -> Vec<f32> {
        let c = &self.config;
        // Continental + ridge share one coarse field, so a single pair of per-axis
        // bilinear factors (wrapped node indices + blend fraction) serves both —
        // hoisted out of the inner loop the same way the detail trig is.
        let lf_x = self.fields.axis_factors(origin_x, cell_size, side);
        let lf_z = self.fields.axis_factors(origin_z, cell_size, side);
        // The detail octave is still 4D noise; hoist its per-axis torus trig.
        let mut trig = vec![(0.0f64, 0.0f64); 2 * side];
        let (detail_x, detail_z) = trig.split_at_mut(side);
        Self::fill_axis_trig(detail_x, c.detail.frequency, 0.0, origin_x, cell_size);
        Self::fill_axis_trig(detail_z, c.detail.frequency, 0.0, origin_z, cell_size);
        let (detail_x, detail_z) = (&*detail_x, &*detail_z);
        let (amp_c, amp_r, amp_d) = (
            c.continental.amplitude,
            c.ridge.amplitude,
            c.detail.amplitude,
        );

        Self::fill_grid(side, |zi, row| {
            let fz = lf_z[zi];
            let (dz0, dz1) = detail_z[zi];
            for (xi, cell) in row.iter_mut().enumerate() {
                let fx = lf_x[xi];
                let broad = self.fields.blend(FieldKind::Continental, fx, fz);
                let ridge_raw = self.fields.blend(FieldKind::Ridge, fx, fz);
                let ridges = 1.0 - ridge_raw.abs();
                let (dx0, dx1) = detail_x[xi];
                let rough = self.detail.get([dx0, dx1, dz0, dz1]) as f32;
                *cell = broad * amp_c + ridges * amp_r + rough * amp_d;
            }
        })
    }

    /// Moisture field for a `side × side` grid, laid out like [`height_grid`](Self::height_grid).
    /// Bit-identical to [`sample_moisture`](Self::sample_moisture) per vertex.
    pub fn moisture_grid(
        &self,
        origin_x: f32,
        origin_z: f32,
        side: usize,
        cell_size: f32,
    ) -> Vec<f32> {
        let c = &self.config;
        // Both moisture octaves share the one coarse field, so a single pair of
        // per-axis bilinear factors serves both — hoisted out of the inner loop.
        let mx = self.fields.axis_factors(origin_x, cell_size, side);
        let mz = self.fields.axis_factors(origin_z, cell_size, side);
        let (base_w, var_w) = (c.moisture_base_weight, c.moisture_variation_weight);

        Self::fill_grid(side, |zi, row| {
            let fz = mz[zi];
            for (xi, cell) in row.iter_mut().enumerate() {
                let fx = mx[xi];
                let base = self.fields.blend(FieldKind::MoistureBase, fx, fz);
                let variation = self.fields.blend(FieldKind::MoistureVariation, fx, fz);
                *cell = ((base * base_w + variation * var_w) * 0.5 + 0.5).clamp(0.0, 1.0);
            }
        })
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
            // Land is "at or above sea level", matching the cell classification.
            if self.sample_height(px, pz) >= sea_level {
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

    /// The grid samplers must reproduce the per-vertex point samplers *bit for
    /// bit*: terrain regenerates while streaming via the point samplers but is
    /// baked via the grid samplers, so any divergence would float or sink plants.
    /// Both now share the precomputed low-frequency field (bilinear) plus an
    /// identical detail-octave trig hoist, so they must still agree exactly.
    /// Compare `f32` bits over a few chunks, including a negative/off-origin one
    /// to exercise the world-wrap arithmetic.
    #[test]
    fn grid_samplers_are_bit_identical_to_point_samplers() {
        use crate::world_core::chunk::{CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS};

        let hm = Heightmap::new(1337, HeightmapConfig::default());
        let side = CHUNK_GRID_RESOLUTION;
        let cell = CHUNK_SIZE_METERS / (side - 1) as f32;

        for &(cx, cz) in &[(0, 0), (5, -3), (200, 17), (-12, 9)] {
            let ox = cx as f32 * CHUNK_SIZE_METERS;
            let oz = cz as f32 * CHUNK_SIZE_METERS;
            let h_grid = hm.height_grid(ox, oz, side, cell);
            let m_grid = hm.moisture_grid(ox, oz, side, cell);
            for idx in 0..side * side {
                let wx = ox + (idx % side) as f32 * cell;
                let wz = oz + (idx / side) as f32 * cell;
                assert_eq!(
                    h_grid[idx].to_bits(),
                    hm.sample_height(wx, wz).to_bits(),
                    "height mismatch at chunk ({cx},{cz}) idx {idx}"
                );
                assert_eq!(
                    m_grid[idx].to_bits(),
                    hm.sample_moisture(wx, wz).to_bits(),
                    "moisture mismatch at chunk ({cx},{cz}) idx {idx}"
                );
            }
        }
    }
}
