//! Precomputed global low-frequency terrain field.
//!
//! Terrain height is the sum of three noise octaves: continental (~10 km
//! wavelength), ridge (~1.1 km), and detail (~55 m); moisture is two more:
//! base (~530 m) and variation (~105 m). Of these five, only the height detail
//! octave is high-frequency relative to the 2 m vertex spacing; the other four
//! barely change within a 256 m chunk and are wildly oversampled when evaluated
//! as 4D noise per vertex.
//!
//! So, exactly like [`RiverField`](crate::world_core::rivers), we bake those
//! four **raw** (pre-amplitude, pre-crease, pre-weight) octaves once onto a
//! coarse grid spanning the whole wrapping world, then per vertex do cheap
//! bilinear lookups instead of 4D evals. The ridge crease (`1 − |n|`), the
//! amplitudes/weights, and the height detail octave are still reconstructed at
//! full resolution per vertex — interpolating the *raw* noise (which is smooth)
//! keeps the height error at the centimetre scale, where interpolating after the
//! crease could not (the V-shaped valley doesn't interpolate). See
//! `docs/CHUNK_GEN_PROFILING.md` (E7).
//!
//! Per-vertex sampling of those four octaves is now a plain bilinear lookup with
//! `rem_euclid` index wrap — no 4D noise eval at all. Seamlessness across the
//! world-wrap boundary is not free from the index wrap alone: it holds because
//! the one-time bake samples the *same* 4D torus mapping the point sampler used
//! (see [`TerrainFields::torus_raw`]), so node 0 and node `res` carry identical
//! values. `rem_euclid` then just wraps the lookup onto that already-periodic
//! grid. The full-res height detail octave still evaluates the 4D torus per
//! vertex.

use std::collections::HashMap;
use std::f64::consts::TAU;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::world_core::chunk::WORLD_SIZE_METERS;
use crate::world_core::noise4::Simplex4;
use crate::world_core::config::HeightmapConfig;

/// Baked raw noise on a coarse global grid, sampled during terrain generation.
/// Immutable and deterministic from `(seed, the octave frequencies/offsets,
/// resolution)`.
///
/// Holds the four low-frequency octaves that barely change within a chunk: the
/// height octaves (continental ~10 km, ridge ~1.1 km) and the moisture octaves
/// (base ~530 m, variation ~105 m). The variation octave is the highest
/// frequency of the four — at the native 32 m/node grid it is sampled ~3×/
/// wavelength (less on web), so it is mildly smoothed by the bilinear lookup.
/// That is acceptable: moisture only feeds biome classification (which is baked
/// once, not regenerated at runtime) and the variation octave carries only a
/// quarter of the moisture weight, so a little lost fine detail just shifts
/// biome boundaries a few metres.
pub struct TerrainFields {
    res: usize,
    /// Per-node raw continental noise (pre-amplitude).
    continental: Vec<f32>,
    /// Per-node raw ridge noise (pre-crease, pre-amplitude).
    ridge: Vec<f32>,
    /// Per-node raw moisture base octave (pre-weight).
    moisture_base: Vec<f32>,
    /// Per-node raw moisture variation octave (pre-weight; baked with its phase
    /// offset, so a plain bilinear lookup reproduces it).
    moisture_variation: Vec<f32>,
}

impl TerrainFields {
    /// Raw value of one toroidal noise octave at world `(x, z)`, matching
    /// [`Heightmap::torus_sample`](crate::world_core::heightmap). The continental,
    /// ridge, and moisture-base octaves sample at zero phase offset; the moisture
    /// variation octave bakes its `(offset_x, offset_z)` phase here so a later
    /// plain bilinear lookup reproduces it. This reproduces the grid nodes the
    /// per-vertex sampler would otherwise compute.
    ///
    /// Must stay in lockstep with `torus_sample`'s mapping; the bit-identity test
    /// in `heightmap.rs` guards the combined result.
    fn torus_raw(
        noise: &Simplex4,
        x: f64,
        z: f64,
        frequency: f64,
        offset_x: f64,
        offset_z: f64,
    ) -> f32 {
        let l = WORLD_SIZE_METERS;
        let k = (frequency * l).round().max(1.0);
        let radius = k / TAU;
        let theta_x = TAU * x / l + offset_x * TAU / k;
        let theta_z = TAU * z / l + offset_z * TAU / k;
        noise.get([
            radius * theta_x.cos(),
            radius * theta_x.sin(),
            radius * theta_z.cos(),
            radius * theta_z.sin(),
        ]) as f32
    }

    /// Build the field by sampling all four raw octaves at every coarse node.
    /// Node `i` sits at world `i * cell` (`cell = L / res`), matching the bilinear
    /// sampler and `RiverField`'s node convention. The per-octave noise seeds
    /// mirror `Heightmap::new` (continental `seed`, ridge `seed+101`, moisture
    /// `seed+1701` for both moisture octaves), so the baked grid matches what the
    /// point sampler would have produced.
    fn build(seed: u32, cfg: &HeightmapConfig) -> Self {
        let res = effective_resolution(cfg);
        let n2 = res * res;
        let cell = WORLD_SIZE_METERS as f32 / res as f32;

        let cont_noise = Simplex4::new(seed);
        let ridge_noise = Simplex4::new(seed.wrapping_add(101));
        let moisture_noise = Simplex4::new(seed.wrapping_add(1701));
        let cont_freq = cfg.continental.frequency;
        let ridge_freq = cfg.ridge.frequency;
        let m_base_freq = cfg.moisture_base_frequency;
        let m_var_freq = cfg.moisture_variation_frequency;
        let m_var_off_x = cfg.moisture_variation_offset_x;
        let m_var_off_z = cfg.moisture_variation_offset_z;

        // Write each octave straight into its own buffer so the bake never
        // materializes an intermediate `Vec` of quads — that would double peak
        // memory (~67 MB temp + ~67 MB final at native 2048²) and add a transpose
        // pass. `node` fills one grid cell across all four buffers.
        let node = |i: usize, c: &mut f32, r: &mut f32, mb: &mut f32, mv: &mut f32| {
            let x = ((i % res) as f32 * cell) as f64;
            let z = ((i / res) as f32 * cell) as f64;
            *c = Self::torus_raw(&cont_noise, x, z, cont_freq, 0.0, 0.0);
            *r = Self::torus_raw(&ridge_noise, x, z, ridge_freq, 0.0, 0.0);
            *mb = Self::torus_raw(&moisture_noise, x, z, m_base_freq, 0.0, 0.0);
            *mv = Self::torus_raw(&moisture_noise, x, z, m_var_freq, m_var_off_x, m_var_off_z);
        };

        let mut continental = vec![0.0f32; n2];
        let mut ridge = vec![0.0f32; n2];
        let mut moisture_base = vec![0.0f32; n2];
        let mut moisture_variation = vec![0.0f32; n2];

        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            continental
                .par_iter_mut()
                .zip(ridge.par_iter_mut())
                .zip(moisture_base.par_iter_mut())
                .zip(moisture_variation.par_iter_mut())
                .enumerate()
                .for_each(|(i, (((c, r), mb), mv))| node(i, c, r, mb, mv));
        }
        #[cfg(target_arch = "wasm32")]
        for i in 0..n2 {
            node(
                i,
                &mut continental[i],
                &mut ridge[i],
                &mut moisture_base[i],
                &mut moisture_variation[i],
            );
        }

        Self {
            res,
            continental,
            ridge,
            moisture_base,
            moisture_variation,
        }
    }

    /// Bilinearly sample the raw `(continental, ridge)` noise at world `(x, z)`,
    /// wrapping toroidally. Identical interpolation math to `RiverField::sample`.
    pub fn sample(&self, x: f32, z: f32) -> (f32, f32) {
        let (ix, iz) = self.bilinear_indices(x, z);
        (
            self.lerp(&self.continental, ix, iz),
            self.lerp(&self.ridge, ix, iz),
        )
    }

    /// Bilinearly sample the raw `(moisture_base, moisture_variation)` octaves at
    /// world `(x, z)`, wrapping toroidally. Weights and the `[0,1]` clamp are
    /// applied by the caller per vertex.
    pub fn sample_moisture(&self, x: f32, z: f32) -> (f32, f32) {
        let (ix, iz) = self.bilinear_indices(x, z);
        (
            self.lerp(&self.moisture_base, ix, iz),
            self.lerp(&self.moisture_variation, ix, iz),
        )
    }

    /// The two wrapped node indices and blend fraction for one axis at world
    /// coordinate `w`. Shared by every per-octave lerp so they all sample the
    /// same cell. Bit-identical to one entry of [`axis_factors`](Self::axis_factors).
    #[inline]
    fn bilinear_indices(&self, x: f32, z: f32) -> ((usize, usize, f32), (usize, usize, f32)) {
        let res = self.res;
        let cell = WORLD_SIZE_METERS as f32 / res as f32;
        let axis = |w: f32| {
            let g = w / cell;
            let g0 = g.floor();
            let i0 = (g0 as i64).rem_euclid(res as i64) as usize;
            (i0, (i0 + 1) % res, g - g0)
        };
        (axis(x), axis(z))
    }

    /// Bilinear blend of one octave buffer at precomputed `(i0, i1, t)` factors.
    /// Identical math to [`blend`](Self::blend) so the point and grid samplers
    /// agree bit-for-bit.
    #[inline]
    fn lerp(
        &self,
        buf: &[f32],
        (ix0, ix1, tx): (usize, usize, f32),
        (iz0, iz1, tz): (usize, usize, f32),
    ) -> f32 {
        let res = self.res;
        let a = buf[iz0 * res + ix0];
        let b = buf[iz0 * res + ix1];
        let c = buf[iz1 * res + ix0];
        let d = buf[iz1 * res + ix1];
        let top = a * (1.0 - tx) + b * tx;
        let bot = c * (1.0 - tx) + d * tx;
        top * (1.0 - tz) + bot * tz
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
    pub fn blend(&self, which: FieldKind, fx: (usize, usize, f32), fz: (usize, usize, f32)) -> f32 {
        let buf = match which {
            FieldKind::Continental => &self.continental,
            FieldKind::Ridge => &self.ridge,
            FieldKind::MoistureBase => &self.moisture_base,
            FieldKind::MoistureVariation => &self.moisture_variation,
        };
        self.lerp(buf, fx, fz)
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
    MoistureBase,
    MoistureVariation,
}

/// Node count per axis the grid is actually built at: the configured resolution
/// floored at 16 so a pathological tiny value still yields a usable grid. Both
/// [`TerrainFields::build`] and [`cache_key`] go through this, so two configs
/// that clamp to the same resolution share one cached field instead of building
/// identical grids under different keys.
fn effective_resolution(cfg: &HeightmapConfig) -> usize {
    (cfg.low_freq_field_resolution as usize).max(16)
}

/// Hash the inputs that determine the grid contents: seed, the baked octaves'
/// frequencies and phase offsets (the torus mapping uses nothing else), and the
/// effective resolution. Amplitudes, the moisture weights, and the detail octave
/// are applied per vertex and don't affect the stored raw grids, so they're
/// excluded — a tweak to them reuses the cached field instead of pointlessly
/// rebuilding it.
fn cache_key(seed: u32, cfg: &HeightmapConfig) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    seed.hash(&mut h);
    cfg.continental.frequency.to_bits().hash(&mut h);
    cfg.ridge.frequency.to_bits().hash(&mut h);
    cfg.moisture_base_frequency.to_bits().hash(&mut h);
    cfg.moisture_variation_frequency.to_bits().hash(&mut h);
    cfg.moisture_variation_offset_x.to_bits().hash(&mut h);
    cfg.moisture_variation_offset_z.to_bits().hash(&mut h);
    effective_resolution(cfg).hash(&mut h);
    h.finish()
}
