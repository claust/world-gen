//! Global river field.
//!
//! Terrain height is otherwise stateless per-coordinate noise, but rivers are a
//! *global* phenomenon: water flows downhill across chunk boundaries and
//! accumulates into channels. So once per world load we solve hydrology over a
//! coarse grid spanning the whole wrapping world and bake a per-cell **carve
//! depth** (metres to subtract from the terrain) and **wetness** (0..1, for the
//! riverbed tint). Per-chunk terrain generation then just bilinearly samples
//! this field — cheap, and globally consistent because every chunk reads the
//! same baked grid.
//!
//! The algorithm is a priority-flood (Barnes 2014): fill depressions while
//! building a drainage tree where every cell points downhill to an outlet (an
//! ocean cell), then accumulate upstream area. Cells with enough upstream area
//! become rivers. The grid wraps toroidally to match the world's noise period.

use crate::world_core::chunk::WORLD_SIZE_METERS;
use crate::world_core::config::GameConfig;
use crate::world_core::heightmap::Heightmap;
#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Plant placement is rejected wherever river wetness exceeds this. Wetness is
/// `0..1` (see [`RiverField`]); the channel itself trends toward `1.0` while
/// merely-damp banks stay low, so this keeps vegetation out of the water without
/// stripping the riparian fringe. Shared by base-flora placement and the spread
/// landing pass so both agree on where "in the river" is.
pub const MAX_PLANTABLE_WETNESS: f32 = 0.15;

/// Upper wetness bound for aquatic plants (cattails). Keeps them out of the deep
/// channel centre so banks read as a reed fringe rather than a carpet over open
/// water. Their lower bound is [`MAX_PLANTABLE_WETNESS`], where land plants stop.
pub const AQUATIC_MAX_WETNESS: f32 = 0.55;

// ---------------------------------------------------------------------------
// Bank-wall shaping.
//
// Carve depth is shaped from a baked *distance-to-nearest-channel* field, not
// from wetness directly. Per terrain vertex the carve is
//   `depth(size) * wall(dist, halfwidth(size))`
// where `wall` holds full depth inside the channel half-width then ramps to
// zero over a fixed few-metre band — a flat bed with steep sides. Distance (in
// metres) interpolates near-linearly between the coarse hydrology cells, unlike
// the thresholded wetness which steps from ~0 to channel-peak in one cell; so
// evaluating the ramp per vertex yields a wall only `WALL_WIDTH` metres wide on
// the 2 m terrain mesh, decoupled from the 32 m grid spacing. Half-width and
// depth both scale with channel size (`size` = nearest channel's normalized
// log-drainage `wet`, 0..1) so creeks stay shallow and narrow while trunk
// rivers cut deep, wide beds.

/// Width (metres) of the bank wall: the band over which carve ramps from full
/// depth down to zero just outside the channel half-width. Smaller = sharper.
const WALL_WIDTH: f32 = 5.0;
/// Channel half-width (metres): the flat-bottomed corridor that carves to full
/// depth. `HALFWIDTH_MIN` at the smallest carving creek, plus `HALFWIDTH_SPAN`
/// more toward a full-size trunk.
const HALFWIDTH_MIN: f32 = 4.0;
const HALFWIDTH_SPAN: f32 = 14.0;
/// Carve depth as a fraction of `max_carve_depth`: `DEPTH_FLOOR` for the
/// smallest creek, plus `DEPTH_SPAN` more toward a full-size trunk.
const DEPTH_FLOOR: f32 = 0.4;
const DEPTH_SPAN: f32 = 0.6;

/// Hermite smoothstep, clamped to `0..1` outside `[edge0, edge1]`.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Cross-channel carve shape at `dist` metres from the nearest channel, given
/// that channel's `size` (0..1). Returns metres to subtract from the terrain:
/// full `depth` inside the half-width, ramping to 0 over [`WALL_WIDTH`] beyond.
fn carve_profile(dist: f32, size: f32, max_carve_depth: f32) -> f32 {
    let depth = max_carve_depth * (DEPTH_FLOOR + DEPTH_SPAN * size);
    let halfwidth = HALFWIDTH_MIN + HALFWIDTH_SPAN * size;
    depth * (1.0 - smoothstep(halfwidth, halfwidth + WALL_WIDTH, dist))
}

/// A baked global river field, sampled during terrain generation.
pub struct RiverField {
    res: usize,
    /// Channel depth (metres) for the largest rivers; the per-vertex carve
    /// scales this by [`carve_profile`].
    max_carve_depth: f32,
    /// Per-cell wetness in `0..1` (riverbed tint strength; also the channel-size
    /// proxy propagated into [`chan_size`](Self::chan_size)).
    wet: Vec<f32>,
    /// Per-cell distance (metres) to the nearest channel cell, capped to the
    /// band the wall profile uses. Bilinearly sampled and fed to [`carve_profile`];
    /// near-linear in metres, so it interpolates a sharp wall the coarse grid
    /// could not represent directly.
    dist: Vec<f32>,
    /// Per-cell size (`0..1`) of the nearest channel — its `wet` — propagated
    /// outward by the same distance transform. Drives the carve depth and
    /// half-width so a bank inherits its channel's scale.
    chan_size: Vec<f32>,
    /// Per-cell normalized downstream flow direction in world space, split into
    /// parallel x/z grids. `(0, 0)` away from rivers; elsewhere a unit vector
    /// pointing the way the water runs (toward the sea). Drives the streaming
    /// animation of the river water surface.
    flow_x: Vec<f32>,
    flow_z: Vec<f32>,
    /// Per-cell water-routing surface elevation (the pit-filled hydrology field).
    /// Sampled as the river water sheet's elevation (minus a freeboard), it is
    /// smooth and locally flat across a channel, so the rendered surface reads as
    /// level water rather than a sheet draped over the carved bed's V-section.
    surf: Vec<f32>,
}

impl RiverField {
    /// An inert field: no carving, no wetness, no flow. Used when rivers are
    /// disabled and as a cheap stand-in in tests.
    pub fn empty() -> Self {
        Self {
            res: 1,
            max_carve_depth: 0.0,
            wet: vec![0.0],
            dist: vec![0.0],
            chan_size: vec![0.0],
            flow_x: vec![0.0],
            flow_z: vec![0.0],
            surf: vec![0.0],
        }
    }

    /// Solve hydrology over the whole world and bake the carve/wetness grids.
    /// Deterministic from `(seed, heightmap config, river config, sea level)` —
    /// the same inputs that feed `gen_key`, so a cached base world never sees a
    /// stale field.
    pub fn generate(seed: u32, config: &GameConfig) -> Self {
        let rc = &config.rivers;
        if !rc.enabled {
            return Self::empty();
        }
        let res = (rc.grid_resolution as usize).max(16);
        let n2 = res * res;
        #[cfg(not(target_arch = "wasm32"))]
        let started = std::time::Instant::now();
        let cell = WORLD_SIZE_METERS as f32 / res as f32;
        let hm = Heightmap::new(seed, config.heightmap.clone());

        // 1. Sample raw terrain height at each grid node, spaced `cell` apart
        //    (parallel). Node i sits at world `i * cell`, matching `sample`.
        let sample_cell = |i: usize| -> f32 {
            let x = (i % res) as f32 * cell;
            let z = (i / res) as f32 * cell;
            hm.sample_height(x, z)
        };
        #[cfg(not(target_arch = "wasm32"))]
        let raw: Vec<f32> = (0..n2).into_par_iter().map(sample_cell).collect();
        #[cfg(target_arch = "wasm32")]
        let raw: Vec<f32> = (0..n2).map(sample_cell).collect();

        // 2. Smooth the routing surface so flow concentrates into trunk rivers
        //    instead of fragmenting across the high-frequency detail noise.
        let route = box_blur_wrapped(
            &raw,
            res,
            rc.smooth_radius as usize,
            rc.smooth_iters as usize,
        );

        // 3. Priority-flood: fill pits, build the drainage tree, accumulate area.
        //    `filled` is the depression-filled routing surface — the level water
        //    would pool/flow to — which we keep (moved, not copied) as the river
        //    sheet's elevation `surf`.
        let Hydrology {
            accum,
            receiver,
            filled: surf,
        } = compute_hydrology(&route, res, config.sea_level);

        // 4. Bake wetness + flow direction from upstream drainage area. Carve
        //    itself is shaped per terrain vertex from the distance field below,
        //    so only the channel-marking wetness is baked here.
        let cell_area = (cell as f64) * (cell as f64);
        let threshold = (rc.min_drainage_area_m2 as f64 / cell_area).max(1.0) as f32;
        let lo = threshold.ln();
        let hi = (threshold * 300.0).ln();
        let mut wet = vec![0.0f32; n2];
        let mut flow_x = vec![0.0f32; n2];
        let mut flow_z = vec![0.0f32; n2];
        let half = res as i64 / 2;
        // Shortest toroidal step between two grid indices on one axis. The
        // receiver is always an 8-neighbour, so the result is in `-1..=1`.
        let wrap_step = |to: i64, from: i64| -> i64 {
            let d = to - from;
            if d > half {
                d - res as i64
            } else if d < -half {
                d + res as i64
            } else {
                d
            }
        };
        for i in 0..n2 {
            let a = accum[i];
            if a <= threshold {
                continue;
            }
            let t = ((a.ln() - lo) / (hi - lo)).clamp(0.0, 1.0);
            wet[i] = t;

            // Downstream direction: from this cell toward its receiver. World x
            // grows with the column index and world z with the row index, so the
            // grid step doubles as the world-space flow direction once normalized.
            let r = receiver[i];
            if r != u32::MAX {
                let dx = wrap_step((r as usize % res) as i64, (i % res) as i64) as f32;
                let dz = wrap_step((r as usize / res) as i64, (i / res) as i64) as f32;
                let len = (dx * dx + dz * dz).sqrt();
                if len > 0.0 {
                    flow_x[i] = dx / len;
                    flow_z[i] = dz / len;
                }
            }
        }

        // 5. Distance-to-channel field. Multi-source Dijkstra from every channel
        //    cell over the wrapping 8-neighbour grid, bounded to the band the
        //    wall profile reaches (carve is 0 beyond it) so only cells near a
        //    channel are touched. Each cell also inherits the size (`wet`) of the
        //    channel it is nearest to, so banks know how deep/wide to cut. The
        //    near-linear distance interpolates a sharp wall the 32 m grid cannot
        //    hold; see [`carve_profile`].
        let n = res as i64;
        let max_dist = HALFWIDTH_MIN + HALFWIDTH_SPAN + WALL_WIDTH + cell;
        let mut dist = vec![max_dist; n2];
        let mut chan_size = vec![0.0f32; n2];
        // Quantize the float distance for the heap ordering (centimetre keys).
        let qkey = |d: f32| (d * 100.0) as i32;
        let mut heap: BinaryHeap<Reverse<(i32, u32)>> = BinaryHeap::new();
        for i in 0..n2 {
            if wet[i] > 0.0 {
                dist[i] = 0.0;
                chan_size[i] = wet[i];
                heap.push(Reverse((0, i as u32)));
            }
        }
        let diag = cell * std::f32::consts::SQRT_2;
        while let Some(Reverse((pk, ci))) = heap.pop() {
            let ci = ci as usize;
            let d0 = dist[ci];
            // Skip stale heap entries: a cell can be pushed at one distance and
            // later improved, leaving the older, larger entry behind. Its key no
            // longer matches the cell's best distance, so there is nothing to do.
            if pk != qkey(d0) {
                continue;
            }
            let cx = (ci % res) as i64;
            let cz = (ci / res) as i64;
            for (dx, dz) in NB8 {
                // Orthogonal steps cost one cell, diagonal steps √2 cells —
                // derived from the offset so this doesn't rely on NB8's order.
                let nd = d0 + if dx != 0 && dz != 0 { diag } else { cell };
                if nd >= max_dist {
                    continue;
                }
                let nx = (cx + dx).rem_euclid(n) as usize;
                let nz = (cz + dz).rem_euclid(n) as usize;
                let ni = nz * res + nx;
                if nd < dist[ni] {
                    dist[ni] = nd;
                    chan_size[ni] = chan_size[ci];
                    heap.push(Reverse((qkey(nd), ni as u32)));
                }
            }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let river_cells = wet.iter().filter(|&&w| w > 0.0).count();
            log::info!(
                "river field: {res}×{res} grid ({:.0} m/cell), {river_cells} river cells \
                 ({:.1}% of world), solved in {:.0} ms",
                WORLD_SIZE_METERS as f32 / res as f32,
                100.0 * river_cells as f32 / n2 as f32,
                started.elapsed().as_secs_f32() * 1000.0,
            );
        }

        Self {
            res,
            max_carve_depth: rc.max_carve_depth,
            wet,
            dist,
            chan_size,
            flow_x,
            flow_z,
            surf,
        }
    }

    /// Bilinearly sample the field at world `(x, z)` (wrapping toroidally).
    /// Returns `(carve_depth_metres, wetness_0_1, water_surface_elevation_m)`.
    /// The surface elevation is the smoothed routing surface; the caller drops a
    /// freeboard off it to place the water sheet below the surrounding banks.
    pub fn sample(&self, x: f32, z: f32) -> (f32, f32, f32) {
        if self.res <= 1 {
            return (0.0, 0.0, 0.0);
        }
        let res = self.res;
        let cell = WORLD_SIZE_METERS as f32 / res as f32;
        // Grid-node coordinates: node i sits at world `i * cell`, so dividing by
        // `cell` puts nodes on integer indices and `tx`/`tz` are the fractions
        // between them.
        let gx = x / cell;
        let gz = z / cell;
        let x0 = gx.floor();
        let z0 = gz.floor();
        let tx = gx - x0;
        let tz = gz - z0;
        let ix0 = (x0 as i64).rem_euclid(res as i64) as usize;
        let iz0 = (z0 as i64).rem_euclid(res as i64) as usize;
        let ix1 = (ix0 + 1) % res;
        let iz1 = (iz0 + 1) % res;

        let lerp = |buf: &[f32]| {
            let a = buf[iz0 * res + ix0];
            let b = buf[iz0 * res + ix1];
            let c = buf[iz1 * res + ix0];
            let d = buf[iz1 * res + ix1];
            let top = a * (1.0 - tx) + b * tx;
            let bot = c * (1.0 - tx) + d * tx;
            top * (1.0 - tz) + bot * tz
        };
        // Carve is shaped here from the *interpolated* distance and channel size
        // rather than baked as a depth and interpolated: the wall profile applied
        // per vertex collapses the bank drop onto a few metres, so the channel
        // reads as a walled cut instead of a wide bowl. See [`carve_profile`].
        let carve = carve_profile(
            lerp(&self.dist),
            lerp(&self.chan_size),
            self.max_carve_depth,
        );
        (carve, lerp(&self.wet), lerp(&self.surf))
    }

    /// Bilinearly sample the downstream flow direction at world `(x, z)`,
    /// renormalized so the caller gets a unit vector (or `(0, 0)` away from any
    /// river). Bilinear blending of neighbouring directions naturally smooths the
    /// flow field; it can shrink near confluences where directions disagree, so
    /// we renormalize rather than trusting the interpolated length.
    pub fn sample_flow(&self, x: f32, z: f32) -> (f32, f32) {
        if self.res <= 1 {
            return (0.0, 0.0);
        }
        let res = self.res;
        let cell = WORLD_SIZE_METERS as f32 / res as f32;
        let gx = x / cell;
        let gz = z / cell;
        let x0 = gx.floor();
        let z0 = gz.floor();
        let tx = gx - x0;
        let tz = gz - z0;
        let ix0 = (x0 as i64).rem_euclid(res as i64) as usize;
        let iz0 = (z0 as i64).rem_euclid(res as i64) as usize;
        let ix1 = (ix0 + 1) % res;
        let iz1 = (iz0 + 1) % res;

        let lerp = |buf: &[f32]| {
            let a = buf[iz0 * res + ix0];
            let b = buf[iz0 * res + ix1];
            let c = buf[iz1 * res + ix0];
            let d = buf[iz1 * res + ix1];
            let top = a * (1.0 - tx) + b * tx;
            let bot = c * (1.0 - tx) + d * tx;
            top * (1.0 - tz) + bot * tz
        };
        let fx = lerp(&self.flow_x);
        let fz = lerp(&self.flow_z);
        let len = (fx * fx + fz * fz).sqrt();
        if len > 1e-4 {
            (fx / len, fz / len)
        } else {
            (0.0, 0.0)
        }
    }
}

/// Separable box blur on a wrapping grid (repeated for an approx. gaussian).
fn box_blur_wrapped(src: &[f32], res: usize, radius: usize, iters: usize) -> Vec<f32> {
    if radius == 0 || iters == 0 {
        return src.to_vec();
    }
    let r = radius as i64;
    let n = res as i64;
    let mut a = src.to_vec();
    let mut b = vec![0.0f32; res * res];
    for _ in 0..iters {
        // horizontal
        for z in 0..res {
            let row = z * res;
            for x in 0..res {
                let mut sum = 0.0;
                for dx in -r..=r {
                    let nx = (x as i64 + dx).rem_euclid(n) as usize;
                    sum += a[row + nx];
                }
                b[row + x] = sum / (2 * radius + 1) as f32;
            }
        }
        // vertical
        for z in 0..res {
            for x in 0..res {
                let mut sum = 0.0;
                for dz in -r..=r {
                    let nz = (z as i64 + dz).rem_euclid(n) as usize;
                    sum += b[nz * res + x];
                }
                a[z * res + x] = sum / (2 * radius + 1) as f32;
            }
        }
    }
    a
}

struct Hydrology {
    accum: Vec<f32>,
    /// Each cell's downhill receiver cell index (`u32::MAX` for outlets/ocean).
    receiver: Vec<u32>,
    /// The routing surface with depressions filled to their pour level — the
    /// elevation standing/flowing water would settle at, used as the river sheet.
    filled: Vec<f32>,
}

const NB8: [(i64, i64); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

/// Priority-flood depression fill + drainage tree + flow accumulation on a
/// toroidally wrapping grid. Outlets are ocean cells (below sea level); if the
/// world has no ocean at all, the single lowest cell is used so flow still
/// terminates somewhere.
fn compute_hydrology(heights: &[f32], res: usize, sea_level: f32) -> Hydrology {
    let n2 = res * res;
    let n = res as i64;
    let mut filled = heights.to_vec();
    let mut visited = vec![false; n2];
    let mut receiver = vec![u32::MAX; n2];
    let key = |h: f32| (h * 1000.0) as i32;
    let mut heap: BinaryHeap<Reverse<(i32, u32)>> = BinaryHeap::new();

    let mut any_ocean = false;
    for i in 0..n2 {
        if heights[i] < sea_level {
            any_ocean = true;
            visited[i] = true;
            heap.push(Reverse((key(filled[i]), i as u32)));
        }
    }
    if !any_ocean {
        // Degenerate world with no sea: drain to the global low point.
        let mut lo = 0usize;
        for i in 1..n2 {
            if heights[i] < heights[lo] {
                lo = i;
            }
        }
        visited[lo] = true;
        heap.push(Reverse((key(filled[lo]), lo as u32)));
    }

    let mut order: Vec<u32> = Vec::with_capacity(n2);
    while let Some(Reverse((_, ci))) = heap.pop() {
        order.push(ci);
        let cx = (ci as usize % res) as i64;
        let cz = (ci as usize / res) as i64;
        for (dx, dz) in NB8 {
            let nx = (cx + dx).rem_euclid(n) as usize;
            let nz = (cz + dz).rem_euclid(n) as usize;
            let ni = nz * res + nx;
            if visited[ni] {
                continue;
            }
            visited[ni] = true;
            if filled[ni] < filled[ci as usize] {
                filled[ni] = filled[ci as usize];
            }
            receiver[ni] = ci;
            heap.push(Reverse((key(filled[ni]), ni as u32)));
        }
    }

    // Accumulate upstream area from leaves to outlets (reverse pop order).
    let mut accum = vec![1.0f32; n2];
    for &ci in order.iter().rev() {
        let r = receiver[ci as usize];
        if r != u32::MAX {
            accum[r as usize] += accum[ci as usize];
        }
    }

    Hydrology {
        accum,
        receiver,
        filled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_core::config::GameConfig;

    #[test]
    fn field_wraps_seamlessly() {
        let mut config = GameConfig::default();
        // Small grid keeps the test fast while still exercising the solver.
        config.rivers.grid_resolution = 128;
        let field = RiverField::generate(42, &config);
        let l = WORLD_SIZE_METERS as f32;
        for &(x, z) in &[(0.0, 0.0), (123.0, 4567.0), (60000.0, 200.0)] {
            let (c0, w0, s0) = field.sample(x, z);
            let (c1, w1, s1) = field.sample(x + l, z - l);
            assert!((c0 - c1).abs() < 1e-3, "carve not periodic at ({x},{z})");
            assert!((w0 - w1).abs() < 1e-3, "wet not periodic at ({x},{z})");
            assert!((s0 - s1).abs() < 1e-3, "surf not periodic at ({x},{z})");
        }
    }

    #[test]
    fn carve_profile_is_a_flat_bottomed_wall() {
        let max = 20.0;
        let size = 0.8;
        let halfwidth = HALFWIDTH_MIN + HALFWIDTH_SPAN * size;
        let depth = max * (DEPTH_FLOOR + DEPTH_SPAN * size);

        // Flat full-depth floor across the whole half-width.
        assert!((carve_profile(0.0, size, max) - depth).abs() < 1e-3);
        assert!((carve_profile(halfwidth, size, max) - depth).abs() < 1e-3);
        // Zero once past the wall, and the transition is confined to WALL_WIDTH.
        assert_eq!(carve_profile(halfwidth + WALL_WIDTH, size, max), 0.0);
        assert_eq!(carve_profile(halfwidth + WALL_WIDTH + 10.0, size, max), 0.0);
        // The wall is steep: most of the drop happens over WALL_WIDTH metres, far
        // sharper than the ~grid-cell-wide ramp a bilinearly-sampled depth gives.
        let mid = carve_profile(halfwidth + 0.5 * WALL_WIDTH, size, max);
        assert!(
            (mid - 0.5 * depth).abs() < 0.05 * depth,
            "wall midpoint {mid}"
        );

        // Depth and half-width both shrink with channel size: a creek cuts a
        // shallower, narrower bed than a trunk.
        assert!(carve_profile(0.0, 0.1, max) < carve_profile(0.0, 0.9, max));
    }

    #[test]
    fn disabled_field_is_inert() {
        let mut config = GameConfig::default();
        config.rivers.enabled = false;
        let field = RiverField::generate(42, &config);
        assert_eq!(field.sample(1234.0, 5678.0), (0.0, 0.0, 0.0));
    }

    #[test]
    fn produces_some_rivers_on_default_world() {
        let mut config = GameConfig::default();
        config.rivers.grid_resolution = 256;
        let field = RiverField::generate(42, &config);
        let wet_cells = field.wet.iter().filter(|&&w| w > 0.0).count();
        assert!(wet_cells > 0, "expected the default world to grow rivers");
    }

    #[test]
    fn flow_is_unit_or_zero_and_wraps() {
        let mut config = GameConfig::default();
        config.rivers.grid_resolution = 256;
        let field = RiverField::generate(42, &config);
        let l = WORLD_SIZE_METERS as f32;

        // Sweep a grid of world points and assert the two invariants of
        // `sample_flow`: every sample is either the zero vector (away from
        // rivers) or unit length (renormalized direction), and it is periodic
        // across the world wrap just like `sample`.
        let mut saw_flow = false;
        for gz in 0..48 {
            for gx in 0..48 {
                let x = gx as f32 / 48.0 * l;
                let z = gz as f32 / 48.0 * l;
                let (fx, fz) = field.sample_flow(x, z);
                let len = (fx * fx + fz * fz).sqrt();
                assert!(
                    len < 1e-3 || (len - 1.0).abs() < 1e-3,
                    "flow at ({x},{z}) is neither zero nor unit: len {len}"
                );
                if len > 0.5 {
                    saw_flow = true;
                }
                let (px, pz) = field.sample_flow(x + l, z - l);
                assert!(
                    (fx - px).abs() < 1e-3 && (fz - pz).abs() < 1e-3,
                    "flow not periodic at ({x},{z})"
                );
            }
        }
        assert!(saw_flow, "expected some river flow on the default world");
    }
}
