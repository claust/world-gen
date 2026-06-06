//! Precomputed global low-frequency terrain field.
//!
//! Terrain height is the sum of three noise octaves: continental (~10 km
//! wavelength), ridge (~1.1 km), and detail (~55 m). Only the detail octave is
//! high-frequency relative to the 2 m vertex spacing; the other two barely
//! change within a 256 m chunk and are wildly oversampled when evaluated as 4D
//! OpenSimplex per vertex.
//!
//! So, exactly like [`RiverField`](crate::world_core::rivers), we bake the
//! **raw** (pre-amplitude, pre-crease) continental and ridge noise once onto a
//! coarse grid spanning the whole wrapping world, then per vertex do two cheap
//! bilinear lookups instead of two 4D evals. The ridge crease (`1 − |n|`) and
//! the detail octave are still reconstructed at full resolution per vertex —
//! interpolating the *raw* noise (which is smooth) keeps the error at the
//! centimetre scale, where interpolating after the crease could not (the
//! V-shaped valley doesn't interpolate). See `docs/CHUNK_GEN_PROFILING.md` (E7).
//!
//! Per-vertex sampling of these two octaves is now a plain bilinear lookup with
//! `rem_euclid` index wrap — no 4D noise eval at all. Seamlessness across the
//! world-wrap boundary is not free from the index wrap alone: it holds because
//! the one-time bake samples the *same* 4D torus mapping the point sampler used
//! (see [`TerrainFields::torus_raw`]), so node 0 and node `res` carry identical
//! values. `rem_euclid` then just wraps the lookup onto that already-periodic
//! grid. The full-res detail octave still evaluates the 4D torus per vertex.

use std::collections::HashMap;
use std::f64::consts::TAU;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use noise::{NoiseFn, OpenSimplex};

use crate::world_core::chunk::WORLD_SIZE_METERS;
use crate::world_core::config::HeightmapConfig;

/// Baked raw continental + ridge noise on a coarse global grid, sampled during
/// terrain generation. Immutable and deterministic from `(seed, continental
/// frequency, ridge frequency, resolution)`.
pub struct TerrainFields {
    res: usize,
    /// Per-node raw continental noise (pre-amplitude).
    continental: Vec<f32>,
    /// Per-node raw ridge noise (pre-crease, pre-amplitude).
    ridge: Vec<f32>,
}

impl TerrainFields {
    /// Raw value of one toroidal noise octave at world `(x, z)`, matching
    /// [`Heightmap::torus_sample`](crate::world_core::heightmap) with zero phase
    /// offset. The continental and ridge octaves both sample at offset 0, so this
    /// reproduces the grid nodes the per-vertex sampler would otherwise compute.
    ///
    /// Must stay in lockstep with `torus_sample`'s mapping; the bit-identity test
    /// in `heightmap.rs` guards the combined result.
    fn torus_raw(noise: &OpenSimplex, x: f64, z: f64, frequency: f64) -> f32 {
        let l = WORLD_SIZE_METERS;
        let k = (frequency * l).round().max(1.0);
        let radius = k / TAU;
        let theta_x = TAU * x / l;
        let theta_z = TAU * z / l;
        noise.get([
            radius * theta_x.cos(),
            radius * theta_x.sin(),
            radius * theta_z.cos(),
            radius * theta_z.sin(),
        ]) as f32
    }

    /// Build the field by sampling raw continental + ridge noise at every coarse
    /// node. Node `i` sits at world `i * cell` (`cell = L / res`), matching the
    /// bilinear sampler and `RiverField`'s node convention.
    fn build(seed: u32, cfg: &HeightmapConfig) -> Self {
        let res = effective_resolution(cfg);
        let n2 = res * res;
        let cell = WORLD_SIZE_METERS as f32 / res as f32;

        let cont_noise = OpenSimplex::new(seed);
        let ridge_noise = OpenSimplex::new(seed.wrapping_add(101));
        let cont_freq = cfg.continental.frequency;
        let ridge_freq = cfg.ridge.frequency;

        let node = |i: usize| -> (f32, f32) {
            let x = ((i % res) as f32 * cell) as f64;
            let z = ((i / res) as f32 * cell) as f64;
            (
                Self::torus_raw(&cont_noise, x, z, cont_freq),
                Self::torus_raw(&ridge_noise, x, z, ridge_freq),
            )
        };

        #[cfg(not(target_arch = "wasm32"))]
        let (continental, ridge): (Vec<f32>, Vec<f32>) = {
            use rayon::prelude::*;
            (0..n2).into_par_iter().map(node).unzip()
        };
        #[cfg(target_arch = "wasm32")]
        let (continental, ridge): (Vec<f32>, Vec<f32>) = (0..n2).map(node).unzip();

        Self {
            res,
            continental,
            ridge,
        }
    }

    /// Bilinearly sample the raw `(continental, ridge)` noise at world `(x, z)`,
    /// wrapping toroidally. Identical interpolation math to `RiverField::sample`.
    pub fn sample(&self, x: f32, z: f32) -> (f32, f32) {
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
        (lerp(&self.continental), lerp(&self.ridge))
    }

    /// Per-axis grid factors so a regular `side × side` sample grid can hoist the
    /// floor/fraction/index work out of the inner loop, mirroring the trig
    /// hoisting in [`Heightmap::height_grid`](crate::world_core::heightmap). Each
    /// returned tuple is `(i0, i1, t)`: the two wrapped node indices to blend and
    /// the blend fraction for a sample at `origin + step * n`.
    pub fn axis_factors(&self, origin: f32, step: f32, count: usize) -> Vec<(usize, usize, f32)> {
        let res = self.res;
        let cell = WORLD_SIZE_METERS as f32 / res as f32;
        (0..count)
            .map(|n| {
                let g = (origin + n as f32 * step) / cell;
                let g0 = g.floor();
                let i0 = (g0 as i64).rem_euclid(res as i64) as usize;
                (i0, (i0 + 1) % res, g - g0)
            })
            .collect()
    }

    /// Blend one octave's grid at precomputed axis factors. `(ix0, ix1, tx)` from
    /// [`axis_factors`](Self::axis_factors) along X, likewise for Z.
    #[inline]
    pub fn blend(
        &self,
        which: FieldKind,
        (ix0, ix1, tx): (usize, usize, f32),
        (iz0, iz1, tz): (usize, usize, f32),
    ) -> f32 {
        let res = self.res;
        let buf = match which {
            FieldKind::Continental => &self.continental,
            FieldKind::Ridge => &self.ridge,
        };
        let a = buf[iz0 * res + ix0];
        let b = buf[iz0 * res + ix1];
        let c = buf[iz1 * res + ix0];
        let d = buf[iz1 * res + ix1];
        let top = a * (1.0 - tx) + b * tx;
        let bot = c * (1.0 - tx) + d * tx;
        top * (1.0 - tz) + bot * tz
    }

    /// Fetch the shared field for these inputs, building it once and memoizing it.
    ///
    /// `Heightmap` is constructed in many places (terrain, rivers, plant
    /// placement, spawn scans) but always from the same `(seed, heightmap
    /// config)` per world, so the ~tens-of-MB grid would otherwise be rebuilt
    /// several times per load. The cache holds a [`Weak`] so the field frees once
    /// the last `Heightmap` using it drops (e.g. on a config reload to new
    /// inputs). Keyed on the inputs that determine the grid contents.
    pub fn shared(seed: u32, cfg: &HeightmapConfig) -> Arc<TerrainFields> {
        static CACHE: OnceLock<Mutex<HashMap<u64, Weak<TerrainFields>>>> = OnceLock::new();
        let key = cache_key(seed, cfg);
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));

        // Fast path: an existing live field for these inputs.
        if let Some(field) = cache.lock().unwrap().get(&key).and_then(Weak::upgrade) {
            return field;
        }
        // Build outside the lock so a slow bake doesn't stall unrelated callers;
        // a rare concurrent double-build on first miss is wasteful but correct.
        let field = Arc::new(Self::build(seed, cfg));
        let mut map = cache.lock().unwrap();
        if let Some(existing) = map.get(&key).and_then(Weak::upgrade) {
            return existing; // someone else won the race; share theirs
        }
        map.insert(key, Arc::downgrade(&field));
        field
    }
}

/// Which baked octave to blend in [`TerrainFields::blend`].
#[derive(Clone, Copy)]
pub enum FieldKind {
    Continental,
    Ridge,
}

/// Node count per axis the grid is actually built at: the configured resolution
/// floored at 16 so a pathological tiny value still yields a usable grid. Both
/// [`TerrainFields::build`] and [`cache_key`] go through this, so two configs
/// that clamp to the same resolution share one cached field instead of building
/// identical grids under different keys.
fn effective_resolution(cfg: &HeightmapConfig) -> usize {
    (cfg.low_freq_field_resolution as usize).max(16)
}

/// Hash the inputs that determine the grid contents: seed, the two octave
/// frequencies (the torus mapping uses nothing else), and the effective
/// resolution. Amplitudes, the detail octave, and moisture are applied per
/// vertex and don't affect the stored raw grids, so they're excluded — a tweak
/// to them reuses the cached field instead of pointlessly rebuilding it.
fn cache_key(seed: u32, cfg: &HeightmapConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h);
    cfg.continental.frequency.to_bits().hash(&mut h);
    cfg.ridge.frequency.to_bits().hash(&mut h);
    effective_resolution(cfg).hash(&mut h);
    h.finish()
}
