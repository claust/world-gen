//! Resident, whole-world plant store.
//!
//! Iteration 1 of the live simulation keeps every plant in every one of the
//! `WORLD_SIZE_CHUNKS²` canonical chunks resident and simulates them on one
//! global clock — see [`docs/LIVE_SIM_ITERATION_1.md`]. This module is the
//! store: a packed `Plant` per canonical chunk, generated once at world
//! creation, read by the render bridge, ticked for growth + spread on the global
//! clock, and persisted as a compact spread-delta on top of the regenerable base.

use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;

use glam::{IVec2, Vec3};

use crate::world_core::biome::{classify, Biome};
use crate::world_core::chunk::{
    canonical_chunk, ChunkTerrain, PlantInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
    WORLD_SIZE_CHUNKS, WORLD_SIZE_METERS,
};
use crate::world_core::chunk_generator::ChunkGenerator;
use crate::world_core::config::{BiomeConfig, GameConfig};
use crate::world_core::content::sampling::{hash4, hash_to_unit_float};
use crate::world_core::heightmap::Heightmap;
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::lifecycle::GrowthStage;
use crate::world_core::rivers::{RiverField, MAX_PLANTABLE_WETNESS};

/// Packed per-plant record — **16 bytes** (`repr(C)`), validated against the
/// real plant count in the M0 feasibility spike.
///
/// Position is stored **chunk-local and quantized** (`u16` over the 256 m chunk,
/// ~3.9 mm step), which makes the store lap-agnostic: a plant renders at any raw
/// chunk by adding that chunk's world origin, so there is no absolute position
/// frozen to one world lap. The ground height `y` is not stored — it is resampled
/// from the loaded chunk's terrain, exactly as base placement computed it.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plant {
    /// Chunk-local x, quantized over `[0, CHUNK_SIZE_METERS)`.
    pub local_x: u16,
    /// Chunk-local z, quantized over `[0, CHUNK_SIZE_METERS)`.
    pub local_z: u16,
    /// Plant height, quantized over `[0, CHUNK_SIZE_METERS)`.
    pub height: u16,
    /// Y-rotation, quantized over `[0, TAU)`.
    pub rotation: u16,
    pub species: u8,
    pub stage: u8,
    pub born_hour: f32,
}

const MATURE: u8 = stage_to_u8(GrowthStage::Mature);

const fn stage_to_u8(stage: GrowthStage) -> u8 {
    match stage {
        GrowthStage::Seedling => 0,
        GrowthStage::Young => 1,
        GrowthStage::Mature => 2,
    }
}

fn stage_from_u8(value: u8) -> GrowthStage {
    match value {
        0 => GrowthStage::Seedling,
        1 => GrowthStage::Young,
        _ => GrowthStage::Mature,
    }
}

// Quantize `[0, CHUNK_SIZE_METERS)` onto the `u16` range using a 2^16 divisor and
// `floor`, so the dequantized value is always strictly less than
// `CHUNK_SIZE_METERS`. A plant therefore never reconstructs exactly on the chunk
// boundary, where `floor(pos / CHUNK_SIZE_METERS)` would attribute it to the
// neighbouring chunk.
const SPAN_STEPS: f32 = 65536.0;

fn quantize_span(value: f32) -> u16 {
    (value / CHUNK_SIZE_METERS * SPAN_STEPS)
        .floor()
        .clamp(0.0, u16::MAX as f32) as u16
}

fn dequantize_span(value: u16) -> f32 {
    value as f32 / SPAN_STEPS * CHUNK_SIZE_METERS
}

fn quantize_rotation(radians: f32) -> u16 {
    (radians.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * u16::MAX as f32)
        .round()
        .clamp(0.0, u16::MAX as f32) as u16
}

impl Plant {
    /// Pack a generated plant whose absolute position lies in the chunk whose
    /// world origin is `(origin_x, origin_z)`.
    ///
    /// Base plants are snapped to the **base-snapshot storage grid** here, so a
    /// world generated locally is bit-identical to one rebuilt from a downloaded
    /// `world_base.bin`: positions to a 10-bit-per-axis grid (`POS_BITS`, 0.25 m),
    /// height to `pack_height` (0.125 m), rotation to 8 bits (~1.4°). Without this
    /// the snapshot would be lossy relative to generation and the two paths would
    /// diverge — and since spread RNG is keyed on a plant's index within its
    /// chunk, the caller also Morton-sorts each chunk after packing.
    fn pack(plant: &PlantInstance, origin_x: f32, origin_z: f32) -> Self {
        let h8 = pack_height(plant.height);
        let r8 = pack_rotation(plant.rotation);
        Plant {
            local_x: snap_pos(quantize_span(plant.position.x - origin_x)),
            local_z: snap_pos(quantize_span(plant.position.z - origin_z)),
            height: unpack_height(h8),
            rotation: (r8 as u16) << 8,
            species: plant.species_index as u8,
            stage: stage_to_u8(plant.growth_stage),
            born_hour: 0.0,
        }
    }

    /// Reconstruct a renderable instance for the raw chunk at `raw_coord`,
    /// resampling ground height from that chunk's terrain.
    fn to_instance(self, raw_coord: IVec2, terrain: &ChunkTerrain) -> PlantInstance {
        let local_x = dequantize_span(self.local_x);
        let local_z = dequantize_span(self.local_z);
        let y = terrain.height_at_world(local_x, local_z);
        PlantInstance {
            position: Vec3::new(
                raw_coord.x as f32 * CHUNK_SIZE_METERS + local_x,
                y,
                raw_coord.y as f32 * CHUNK_SIZE_METERS + local_z,
            ),
            rotation: self.rotation as f32 / u16::MAX as f32 * std::f32::consts::TAU,
            height: dequantize_span(self.height),
            species_index: self.species as usize,
            growth_stage: stage_from_u8(self.stage),
        }
    }
}

/// Per-canonical-chunk packed plant lists for the whole finite world.
pub struct PlantWorld {
    /// One list per canonical chunk, indexed `cz * WORLD_SIZE_CHUNKS + cx`.
    chunks: Vec<Vec<Plant>>,
    /// Count of plants not yet `Mature`, per chunk — lets the growth tick skip
    /// fully-grown chunks.
    immature: Vec<u32>,
    /// Per chunk: all-mature and saturated — the last spread pass added nothing
    /// here. A chunk's plants can only spread into the 8 adjacent chunks (spread
    /// radius ≪ chunk size), so phase 1 skips a chunk only when it *and* all eight
    /// neighbours are saturated. That keeps a packed chunk feeding a still-filling
    /// neighbour, while making the global pass cheap once a whole region fills.
    saturated: Vec<bool>,
    /// Number of deterministic base-flora plants per chunk, always the prefix of
    /// each chunk's list. Persistence saves only the spread suffix beyond this,
    /// since the base is regenerable from the seed.
    base_count: Vec<u32>,
    /// Biome at each chunk's centre, for per-biome telemetry.
    biome: Vec<Biome>,
    /// Per-chunk overview-map cell byte (biome + water override), the same value
    /// fed to the loading visualization. Retained so the base-world snapshot can
    /// repaint the map on a cache load without regenerating terrain.
    cell_bytes: Vec<u8>,
    registry: Arc<PlantRegistry>,
    /// Terrain + rules for validating spread landings without the full per-chunk
    /// grid: the heightmap is point-sampled at each seedling position.
    heightmap: Heightmap,
    /// Global river field, retained so the spread landing pass can reject
    /// seedlings that fall in a river channel — the same guard base flora
    /// applies via the per-chunk `terrain.river` grid.
    rivers: Arc<RiverField>,
    biome_config: BiomeConfig,
    sea_level: f32,
    seed: u32,
    /// Cached totals so `stats()` (called every frame) stays O(1), maintained as
    /// spread adds plants and transitions chunks empty→non-empty.
    population: usize,
    populated_chunks: usize,
    /// Seedlings added by the most recent spread pass, for telemetry.
    last_spread_added: usize,
    /// Cached per-biome fill, refreshed only when saturation changes — `stats()`
    /// reads it every frame, so it must not rescan the world each call.
    biome_fill: Vec<(&'static str, f32, usize)>,
}

fn chunk_index(canon: IVec2) -> usize {
    (canon.y * WORLD_SIZE_CHUNKS + canon.x) as usize
}

impl PlantWorld {
    /// Generate base flora for every canonical chunk in parallel. Terrain and
    /// biome maps are produced transiently per chunk and discarded once the
    /// plants are packed. This is the one-time world-creation cost.
    pub fn generate_base(
        seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
        threads: usize,
        progress: Option<&crate::world_runtime::gen_progress::GenerationProgress>,
    ) -> Self {
        // `Plant` packs the species index into a `u8`; guarantee the herbarium
        // fits before generating millions of plants (this is a long-lived store).
        assert!(
            registry.species.len() <= u8::MAX as usize + 1,
            "PlantWorld packs species_index into a u8, but the herbarium has {} species (max 256)",
            registry.species.len()
        );
        // The saturation skip assumes a plant spreads no further than an adjacent
        // chunk, so seedlings reach at most the 8-neighbourhood.
        let max_spread = registry
            .species
            .iter()
            .map(|s| s.placement.spread_radius)
            .fold(0.0f32, f32::max);
        assert!(
            max_spread < CHUNK_SIZE_METERS,
            "spread radius {max_spread} must stay within one chunk ({CHUNK_SIZE_METERS} m)"
        );

        let n = WORLD_SIZE_CHUNKS;
        let total = (n as usize) * (n as usize);
        let generator =
            ChunkGenerator::new(seed, config, Arc::clone(&registry), Arc::clone(&rivers));

        // Pack one canonical chunk's base flora (and classify its centre biome);
        // terrain/biome maps are transient.
        let centre =
            (CHUNK_GRID_RESOLUTION / 2) * CHUNK_GRID_RESOLUTION + CHUNK_GRID_RESOLUTION / 2;
        let sea_level = config.sea_level;
        let build_one = |idx: usize| -> (Vec<Plant>, Biome, u8) {
            let cx = (idx as i32) % n;
            let cz = (idx as i32) / n;
            let coord = IVec2::new(cx, cz);
            let data = generator.generate_chunk(coord);
            let origin_x = cx as f32 * CHUNK_SIZE_METERS;
            let origin_z = cz as f32 * CHUNK_SIZE_METERS;
            let mut plants = Vec::with_capacity(data.content.base_plants.len());
            for plant in &data.content.base_plants {
                plants.push(Plant::pack(plant, origin_x, origin_z));
            }
            // Canonical Morton order: makes the base-snapshot position deltas
            // small (the on-disk win) and fixes each plant's index within the
            // chunk, which spread RNG is keyed on — so a downloaded snapshot and a
            // local regeneration produce the same world and evolve identically.
            sort_chunk_canonical(&mut plants);
            plants.shrink_to_fit();
            let centre_height = data.terrain.heights[centre];
            let biome = classify(centre_height, data.terrain.moisture[centre], &config.biome);
            let cell =
                crate::world_runtime::gen_progress::cell_byte(biome, centre_height < sea_level);
            // Surface live progress for the loading visualization: store this
            // chunk's cell color and bump the done counter (single relaxed atomic
            // each — no lock on the generation hot path).
            if let Some(p) = progress {
                p.record(idx, cell);
            }
            (plants, biome, cell)
        };

        #[cfg(not(target_arch = "wasm32"))]
        let built: Vec<(Vec<Plant>, Biome, u8)> = {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads.max(1))
                .build()
                .ok();
            let run = || {
                (0..total)
                    .into_par_iter()
                    .map(build_one)
                    .collect::<Vec<_>>()
            };
            match &pool {
                Some(pool) => pool.install(run),
                None => run(),
            }
        };
        #[cfg(target_arch = "wasm32")]
        let built: Vec<(Vec<Plant>, Biome, u8)> = {
            let _ = threads;
            (0..total).map(build_one).collect()
        };

        let mut chunks = Vec::with_capacity(total);
        let mut biome = Vec::with_capacity(total);
        let mut cell_bytes = Vec::with_capacity(total);
        for (plants, b, cell) in built {
            chunks.push(plants);
            biome.push(b);
            cell_bytes.push(cell);
        }

        let population = chunks.iter().map(Vec::len).sum();
        let populated_chunks = chunks.iter().filter(|c| !c.is_empty()).count();
        let immature = chunks
            .iter()
            .map(|c| c.iter().filter(|p| p.stage != MATURE).count() as u32)
            .collect();
        let base_count = chunks.iter().map(|c| c.len() as u32).collect();
        let saturated = vec![false; chunks.len()];

        let mut world = Self {
            chunks,
            immature,
            saturated,
            base_count,
            biome,
            cell_bytes,
            registry,
            heightmap: Heightmap::new(seed, config.heightmap.clone()),
            rivers,
            biome_config: config.biome.clone(),
            sea_level: config.sea_level,
            seed,
            population,
            populated_chunks,
            last_spread_added: 0,
            biome_fill: Vec::new(),
        };
        world.recompute_biome_fill();
        world
    }

    /// Repaint a [`GenerationProgress`] map from this world's stored cell bytes
    /// and mark every chunk done. Used on the base-snapshot cache path, where the
    /// per-chunk map was loaded rather than recomputed during generation.
    pub fn paint_progress(
        &self,
        progress: &crate::world_runtime::gen_progress::GenerationProgress,
    ) {
        for (idx, &byte) in self.cell_bytes.iter().enumerate() {
            progress.record(idx, byte);
        }
    }

    /// Render a high-resolution, relief-shaded world-map image (RGBA8,
    /// `res`×`res`) by resampling the retained continuous heightmap. Used for the
    /// static in-game map overlay (`M`), which would otherwise reuse the blocky
    /// one-cell-per-chunk loading map.
    pub fn render_world_map(&self, res: usize) -> Vec<u8> {
        crate::world_runtime::world_map::render_world_map(&self.heightmap, self.sea_level, res)
    }

    pub fn sample_height(&self, x: f32, z: f32) -> f32 {
        self.heightmap.sample_height(x, z)
    }

    /// Total plants across the whole world (loaded or not). O(1).
    pub fn population(&self) -> usize {
        self.population
    }

    /// Number of canonical chunks holding at least one plant. O(1).
    pub fn populated_chunks(&self) -> usize {
        self.populated_chunks
    }

    /// Packed plants for a canonical chunk.
    fn chunk(&self, canon: IVec2) -> &[Plant] {
        &self.chunks[chunk_index(canon)]
    }

    /// Reconstruct renderable instances for the raw chunk at `raw_coord`, reading
    /// the canonical chunk's plant list and resampling ground height from
    /// `terrain`.
    pub fn instances_for(&self, raw_coord: IVec2, terrain: &ChunkTerrain) -> Vec<PlantInstance> {
        let canon = canonical_chunk(raw_coord);
        self.chunk(canon)
            .iter()
            .map(|plant| plant.to_instance(raw_coord, terrain))
            .collect()
    }

    /// Advance growth on the global clock. Growth is analytic — each plant's
    /// stage is a function of its `born_hour` and the species' stage durations —
    /// so this only walks chunks that still hold immature plants. Returns whether
    /// any plant's stage advanced (so the render bridge can refresh loaded chunks).
    pub fn tick_growth(&mut self, total_hours: f64) -> bool {
        let registry = &self.registry;
        let mut changed = false;
        for (idx, chunk) in self.chunks.iter_mut().enumerate() {
            if self.immature[idx] == 0 {
                continue;
            }
            let mut immature = 0u32;
            for plant in chunk.iter_mut() {
                if plant.stage != MATURE {
                    let next = stage_to_u8(stage_for(plant, total_hours, registry));
                    if next != plant.stage {
                        plant.stage = next;
                        changed = true;
                    }
                    if plant.stage != MATURE {
                        immature += 1;
                    }
                }
            }
            self.immature[idx] = immature;
        }
        changed
    }

    /// Global spread pass on the canonical grid. Two-phase to stay race-free:
    /// phase 1 (parallel, read-only) has every mature plant emit seedling
    /// candidates targeting a canonical chunk; phase 2 validates each candidate
    /// against the target chunk's terrain (point-sampled) and existing plants
    /// (spacing) and appends the survivors. Spacing is the capacity gate, so the
    /// population grows toward a bounded full state and then stops. Returns
    /// whether any seedling was added.
    pub fn tick_spread(&mut self, born_hour: f64) -> bool {
        let total = self.chunks.len();

        // Phase 1: emit candidates. Read-only over the whole grid. A chunk is
        // skipped only when it and all eight neighbours are saturated, so a packed
        // chunk still seeds a not-yet-full neighbour across the border.
        let saturated = &self.saturated;
        let emit = |idx: usize| {
            if neighbourhood_saturated(saturated, idx) {
                return Vec::new();
            }
            emit_chunk_candidates(idx, &self.chunks, &self.registry, self.seed)
        };
        #[cfg(not(target_arch = "wasm32"))]
        let candidates: Vec<SpreadCandidate> = (0..total).into_par_iter().flat_map(emit).collect();
        #[cfg(target_arch = "wasm32")]
        let candidates: Vec<SpreadCandidate> = (0..total).flat_map(emit).collect();

        if candidates.is_empty() {
            return false;
        }

        // Scatter candidates to their target chunk.
        let mut incoming: Vec<Vec<SpreadCandidate>> = (0..total).map(|_| Vec::new()).collect();
        for candidate in candidates {
            incoming[candidate.target as usize].push(candidate);
        }

        // Phase 2: validate + append, one chunk per task (chunk Vecs are disjoint).
        let ctx = SpreadContext {
            heightmap: &self.heightmap,
            rivers: &self.rivers,
            registry: &self.registry,
            biome_config: &self.biome_config,
            sea_level: self.sea_level,
            born_hour: born_hour as f32,
        };
        let chunks = &mut self.chunks;
        let landed = |(plants, cands): (&mut Vec<Plant>, &mut Vec<SpreadCandidate>)| {
            land_chunk_candidates(plants, cands, &ctx)
        };
        #[cfg(not(target_arch = "wasm32"))]
        let accepted: Vec<u32> = chunks
            .par_iter_mut()
            .zip(incoming.par_iter_mut())
            .map(landed)
            .collect();
        #[cfg(target_arch = "wasm32")]
        let accepted: Vec<u32> = chunks
            .iter_mut()
            .zip(incoming.iter_mut())
            .map(landed)
            .collect();

        // Serial fold of the per-chunk results into the cached counters and the
        // saturation flags.
        let mut added = 0usize;
        for (idx, &count) in accepted.iter().enumerate() {
            if count > 0 {
                added += count as usize;
                // It gained plants, so it is not saturated and may spread once
                // those seedlings mature.
                self.saturated[idx] = false;
                // `len() == count` means the chunk held nothing before this pass,
                // so it just transitioned empty → non-empty.
                if self.chunks[idx].len() == count as usize {
                    self.populated_chunks += 1;
                }
                self.immature[idx] += count;
                self.population += count as usize;
            } else {
                // Nothing landed: saturated iff nothing is left growing here. A
                // neighbour maturing and spreading in (above) clears this again.
                self.saturated[idx] = self.immature[idx] == 0;
            }
        }
        self.last_spread_added = added;
        // Saturation may have flipped this pass; refresh the cached telemetry.
        self.recompute_biome_fill();
        added > 0
    }

    /// Seedlings added by the most recent spread pass.
    pub fn last_spread_added(&self) -> usize {
        self.last_spread_added
    }

    /// Approximate resident bytes of the store (packed plants + per-chunk Vec
    /// headers + the parallel index Vecs).
    pub fn resident_bytes(&self) -> usize {
        let plant = self.population * std::mem::size_of::<Plant>();
        let headers = self.chunks.len() * std::mem::size_of::<Vec<Plant>>();
        let side = self.chunks.len();
        let indices = side
            * (std::mem::size_of::<u32>() // immature
                + std::mem::size_of::<bool>() // saturated
                + std::mem::size_of::<u32>() // base_count
                + std::mem::size_of::<Biome>()); // biome
        plant + headers + indices
    }

    /// Per-biome fill: the percentage (0–100) of each biome's chunks that are
    /// saturated (done spreading), as `(biome_name, percent, chunk_count)` for the
    /// five biomes; biomes with no chunks report 0. O(1): it returns a cache
    /// refreshed only when saturation can change (spread passes, load, creation),
    /// since `stats()` reads it every frame.
    pub fn biome_fill_percents(&self) -> Vec<(&'static str, f32, usize)> {
        self.biome_fill.clone()
    }

    /// Recompute the per-biome fill cache in one pass. Called after generation,
    /// load, and any spread pass that can flip saturation.
    fn recompute_biome_fill(&mut self) {
        let mut total = [0usize; BIOME_SLOTS];
        let mut saturated = [0usize; BIOME_SLOTS];
        for (idx, &b) in self.biome.iter().enumerate() {
            let slot = biome_slot(b);
            total[slot] += 1;
            if self.saturated[idx] {
                saturated[slot] += 1;
            }
        }
        self.biome_fill = BIOME_ORDER
            .iter()
            .map(|&biome| {
                let slot = biome_slot(biome);
                let percent = if total[slot] == 0 {
                    0.0
                } else {
                    saturated[slot] as f32 / total[slot] as f32 * 100.0
                };
                (biome_name(biome), percent, total[slot])
            })
            .collect();
    }

    /// Save the spread suffix of every chunk (the plants beyond the regenerable
    /// base) as a compact binary blob. Round-trips the full world together with
    /// the seed-deterministic base.
    pub fn save_spread(
        &self,
        storage: &dyn crate::world_core::storage::Storage,
    ) -> anyhow::Result<()> {
        let spread_chunks: Vec<usize> = (0..self.chunks.len())
            .filter(|&i| self.chunks[i].len() as u32 > self.base_count[i])
            .collect();

        // header(16) + per-chunk(8) + 14 bytes per spread plant.
        let plant_total: usize = spread_chunks
            .iter()
            .map(|&i| self.chunks[i].len() - self.base_count[i] as usize)
            .sum();
        let mut buf = Vec::with_capacity(16 + spread_chunks.len() * 8 + plant_total * 14);
        buf.extend_from_slice(&SPREAD_MAGIC.to_le_bytes());
        buf.extend_from_slice(&SPREAD_VERSION.to_le_bytes());
        buf.extend_from_slice(&self.seed.to_le_bytes());
        buf.extend_from_slice(&(spread_chunks.len() as u32).to_le_bytes());
        for &i in &spread_chunks {
            let suffix = &self.chunks[i][self.base_count[i] as usize..];
            buf.extend_from_slice(&(i as u32).to_le_bytes());
            buf.extend_from_slice(&(suffix.len() as u32).to_le_bytes());
            for plant in suffix {
                write_plant(&mut buf, plant);
            }
        }
        storage.save_bytes("plants", &buf)
    }

    /// Re-apply a previously saved spread suffix on top of the freshly generated
    /// base. Ignored (with a warning) if absent, malformed, or for another seed.
    /// Returns how many plants were restored.
    pub fn apply_saved_spread(
        &mut self,
        storage: &dyn crate::world_core::storage::Storage,
    ) -> usize {
        self.apply_saved_spread_bytes(storage.load_bytes("plants").as_deref())
    }

    /// As [`apply_saved_spread`], but from an already-loaded blob. The async
    /// generation path reads the save bytes on the main thread (storage handles
    /// are not `Send`) and hands them to the worker through this.
    pub fn apply_saved_spread_bytes(&mut self, bytes: Option<&[u8]>) -> usize {
        let Some(buf) = bytes else {
            return 0;
        };
        match self.read_spread(buf) {
            Ok(added) => added,
            Err(err) => {
                log::warn!("ignoring saved spread state: {err}");
                0
            }
        }
    }

    fn read_spread(&mut self, buf: &[u8]) -> anyhow::Result<usize> {
        let mut cur = Cursor::new(buf);
        if cur.read_u32()? != SPREAD_MAGIC {
            anyhow::bail!("bad magic");
        }
        if cur.read_u32()? != SPREAD_VERSION {
            anyhow::bail!("unsupported version");
        }
        let seed = cur.read_u32()?;
        if seed != self.seed {
            anyhow::bail!("seed {seed} does not match world seed {}", self.seed);
        }
        let chunk_count = cur.read_u32()? as usize;
        let mut added = 0usize;
        for _ in 0..chunk_count {
            let idx = cur.read_u32()? as usize;
            let count = cur.read_u32()? as usize;
            if idx >= self.chunks.len() {
                anyhow::bail!("chunk index {idx} out of range");
            }
            // Bound `count` by the bytes actually left before reserving, so a
            // corrupted length can't trigger a huge allocation ahead of the EOF.
            if count.saturating_mul(PLANT_BYTES) > cur.remaining() {
                anyhow::bail!("declared {count} plants exceed the remaining data");
            }
            // The base list was shrunk to fit during generation; reserve the
            // suffix up front so restoring it is one growth, not many.
            self.chunks[idx].reserve(count);
            for _ in 0..count {
                let plant = read_plant(&mut cur)?;
                if plant.stage != MATURE {
                    self.immature[idx] += 1;
                }
                self.chunks[idx].push(plant);
                added += 1;
            }
        }
        // Recompute cached totals from scratch — cheap next to world generation.
        self.population = self.chunks.iter().map(Vec::len).sum();
        self.populated_chunks = self.chunks.iter().filter(|c| !c.is_empty()).count();
        // Seed saturation so a resumed, settled world is cheap on its first spread
        // pass instead of emitting for every chunk: an all-mature chunk is treated
        // as saturated (a chunk that still has room re-clears the flag the moment a
        // neighbour's seedling lands in it).
        for (idx, &immature) in self.immature.iter().enumerate() {
            self.saturated[idx] = immature == 0;
        }
        self.recompute_biome_fill();
        Ok(added)
    }

    /// Brotli quality for the snapshot shipped on GitHub and downloaded by
    /// clients: a high level (~31 MiB, near the practical size floor — q11 saves
    /// only ~1 MiB for ~20 s more) at ~1 min to encode, paid once in CI and
    /// never on a player's machine. See [`serialize_base`](Self::serialize_base).
    pub const DOWNLOAD_QUALITY: u32 = 10;
    /// Brotli quality for a locally generated cache: ~30% smaller than raw
    /// (~67→47 MiB) in well under a second, so a New Game pays no perceptible
    /// compression cost. Higher levels hit sharp diminishing returns — matching
    /// the download's size would cost the full ~1 min encode for ~16 MiB more.
    pub const LOCAL_QUALITY: u32 = 1;

    /// Serialize the whole base world — every canonical chunk's full plant list
    /// plus its biome and overview-map cell — as a compact binary blob, tagged
    /// with `gen_key` (a hash of the generation inputs) and the seed. This is the
    /// expensive [`generate_base`] output cached so a later New Game can skip
    /// regeneration. Distinct from [`save_spread`], which persists only the
    /// per-game spread delta on top of a regenerable base.
    ///
    /// Layout (v2): a small plaintext header, then three independently
    /// Brotli-compressed columnar sections. Splitting by field lets the
    /// compressor exploit each column's statistics — `meta` (biome/cell/count)
    /// and `attr` (height/rotation/species) are low-entropy and collapse, while
    /// `pos` (the irreducible position core) is kept small by Morton-sorting each
    /// chunk and storing varint deltas of the sorted codes. Plants are assumed to
    /// already be in canonical Morton order (generation and load both sort), so
    /// deltas are non-negative.
    ///
    /// `quality` is the Brotli level (0–11) for each section. The prebuilt
    /// snapshot shipped on GitHub uses [`Self::DOWNLOAD_QUALITY`] (q10) so the
    /// once-in-CI encode buys the smallest practical download; a locally
    /// generated cache uses [`Self::LOCAL_QUALITY`] (q1), which is ~30% smaller
    /// than raw for a fraction of a second — q10 would add ~minutes to every New
    /// Game for a file that never leaves the disk. Brotli decode is
    /// quality-agnostic, so every quality yields the same v2 layout (only the
    /// compressed bytes differ) and loads through the identical path.
    pub fn serialize_base(&self, gen_key: u64, quality: u32) -> Vec<u8> {
        let total = self.chunks.len();

        let mut meta = Vec::with_capacity(total * 6);
        let mut pos = Vec::new();
        let mut attr = Vec::new();
        for idx in 0..total {
            let plants = &self.chunks[idx];
            meta.push(biome_slot(self.biome[idx]) as u8);
            meta.push(self.cell_bytes[idx]);
            meta.extend_from_slice(&(plants.len() as u32).to_le_bytes());
            let mut prev = 0u32;
            for p in plants {
                let code = morton10(p.local_x >> POS_SHIFT, p.local_z >> POS_SHIFT);
                // Canonical Morton order is a hard precondition of the delta
                // encoding, so fail fast in every build rather than wrapping into
                // a corrupt position stream that would only surface at load.
                assert!(code >= prev, "base chunk not in canonical Morton order");
                write_varint(&mut pos, code - prev);
                prev = code;
                attr.push(pack_height(dequantize_span(p.height)));
                attr.push((p.rotation >> 8) as u8);
                attr.push(p.species);
            }
        }

        let mut buf = Vec::new();
        buf.extend_from_slice(&BASE_MAGIC.to_le_bytes());
        buf.extend_from_slice(&BASE_VERSION.to_le_bytes());
        buf.extend_from_slice(&gen_key.to_le_bytes());
        buf.extend_from_slice(&self.seed.to_le_bytes());
        buf.extend_from_slice(&(WORLD_SIZE_CHUNKS as u32).to_le_bytes());
        buf.push(POS_BITS as u8);
        buf.extend_from_slice(&[0u8; 3]); // reserved (keeps the header word-aligned)
        write_section(&mut buf, &meta, quality);
        write_section(&mut buf, &pos, quality);
        write_section(&mut buf, &attr, quality);
        buf
    }

    /// Reconstruct a base world from a [`serialize_base`] blob, rebuilding the
    /// derived indices (immature counts, base counts, totals, biome fill). The
    /// snapshot is accepted only when its magic, version, generation key, seed,
    /// and world size all match the current world; on any mismatch or corruption
    /// it returns `None` (with a warning) so the caller falls back to generation.
    pub fn from_base_snapshot(
        bytes: &[u8],
        gen_key: u64,
        expected_seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
    ) -> Option<Self> {
        match Self::read_base(bytes, gen_key, expected_seed, config, registry, rivers) {
            Ok(world) => Some(world),
            Err(err) => {
                log::warn!("ignoring cached base world: {err}");
                None
            }
        }
    }

    fn read_base(
        buf: &[u8],
        gen_key: u64,
        expected_seed: u32,
        config: &GameConfig,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
    ) -> anyhow::Result<Self> {
        let mut cur = Cursor::new(buf);
        if cur.read_u32()? != BASE_MAGIC {
            anyhow::bail!("bad magic");
        }
        if cur.read_u32()? != BASE_VERSION {
            anyhow::bail!("unsupported version");
        }
        let key = cur.read_u64()?;
        if key != gen_key {
            anyhow::bail!("generation key {key:#018x} does not match {gen_key:#018x}");
        }
        let seed = cur.read_u32()?;
        if seed != expected_seed {
            anyhow::bail!("seed {seed} does not match world seed {expected_seed}");
        }
        let n = cur.read_u32()?;
        if n != WORLD_SIZE_CHUNKS as u32 {
            anyhow::bail!("world size {n} does not match {WORLD_SIZE_CHUNKS}");
        }
        let pos_bits = cur.read_u8()?;
        if pos_bits as u32 != POS_BITS {
            anyhow::bail!("position precision {pos_bits} does not match {POS_BITS}");
        }
        cur.skip(3)?; // reserved
        let total = (n as usize) * (n as usize);

        // Three Brotli sections, decompressed into their own buffers and walked
        // in lockstep: `meta` drives the per-chunk loop, `pos`/`attr` feed each
        // plant. Section lengths are bounded inside `read_section`. Decode is
        // independent of the quality each was written at.
        let meta = read_section(&mut cur)?;
        let pos = read_section(&mut cur)?;
        let attr = read_section(&mut cur)?;
        let mut meta_cur = Cursor::new(&meta);
        let mut pos_cur = Cursor::new(&pos);
        let mut attr_cur = Cursor::new(&attr);

        let mut chunks: Vec<Vec<Plant>> = Vec::with_capacity(total);
        let mut biome = Vec::with_capacity(total);
        let mut cell_bytes = Vec::with_capacity(total);
        for _ in 0..total {
            let biome_code = meta_cur.read_u8()?;
            let cell = meta_cur.read_u8()?;
            let count = meta_cur.read_u32()? as usize;
            // Each plant consumes at least one varint byte from `pos` and exactly
            // three from `attr`; bound `count` by both before reserving so a
            // corrupted length can't trigger a huge allocation ahead of EOF. Also
            // cap by the number of distinct Morton cells — a 256 m chunk on the
            // 10-bit grid can't hold more, and base plants are metres apart, so
            // this never rejects real data but stops a crafted count from driving
            // a multi-gigabyte `with_capacity`.
            if count > MORTON_CELLS
                || count > pos_cur.remaining()
                || count.saturating_mul(3) > attr_cur.remaining()
            {
                anyhow::bail!("declared {count} plants exceed the section data");
            }
            let mut plants = Vec::with_capacity(count);
            let mut prev = 0u32;
            for _ in 0..count {
                let delta = read_varint(&mut pos_cur)?;
                let code = prev
                    .checked_add(delta)
                    .ok_or_else(|| anyhow::anyhow!("position code overflow"))?;
                prev = code;
                // `unmorton10` only reads the low `2*POS_BITS` bits, so reject any
                // out-of-range code rather than silently wrapping it to a bogus
                // in-range position.
                if code as usize >= MORTON_CELLS {
                    anyhow::bail!("position code {code} out of range");
                }
                let (x10, z10) = unmorton10(code);
                let h8 = attr_cur.read_u8()?;
                let r8 = attr_cur.read_u8()?;
                let species = attr_cur.read_u8()?;
                plants.push(Plant {
                    local_x: x10 << POS_SHIFT,
                    local_z: z10 << POS_SHIFT,
                    height: unpack_height(h8),
                    rotation: (r8 as u16) << 8,
                    species,
                    stage: MATURE,
                    born_hour: 0.0,
                });
            }
            chunks.push(plants);
            biome.push(biome_from_slot(biome_code));
            cell_bytes.push(cell);
        }

        let population = chunks.iter().map(Vec::len).sum();
        let populated_chunks = chunks.iter().filter(|c| !c.is_empty()).count();
        let immature = chunks
            .iter()
            .map(|c| c.iter().filter(|p| p.stage != MATURE).count() as u32)
            .collect();
        let base_count = chunks.iter().map(|c| c.len() as u32).collect();
        let saturated = vec![false; chunks.len()];

        let mut world = Self {
            chunks,
            immature,
            saturated,
            base_count,
            biome,
            cell_bytes,
            registry,
            heightmap: Heightmap::new(seed, config.heightmap.clone()),
            rivers,
            biome_config: config.biome.clone(),
            sea_level: config.sea_level,
            seed,
            population,
            populated_chunks,
            last_spread_added: 0,
            biome_fill: Vec::new(),
        };
        world.recompute_biome_fill();
        Ok(world)
    }
}

/// True when chunk `idx` and all eight of its (wrapped) neighbours are saturated,
/// so the chunk's plants cannot place a seedling anywhere this pass.
fn neighbourhood_saturated(saturated: &[bool], idx: usize) -> bool {
    if !saturated[idx] {
        return false;
    }
    let n = WORLD_SIZE_CHUNKS;
    let cx = (idx as i32) % n;
    let cz = (idx as i32) / n;
    for dz in -1..=1 {
        for dx in -1..=1 {
            if dx == 0 && dz == 0 {
                continue;
            }
            let nx = (cx + dx).rem_euclid(n);
            let nz = (cz + dz).rem_euclid(n);
            if !saturated[(nz * n + nx) as usize] {
                return false;
            }
        }
    }
    true
}

/// A seedling proposed by phase 1, targeting canonical chunk `target`.
struct SpreadCandidate {
    target: u32,
    /// Position local to the target chunk, in metres `[0, CHUNK_SIZE_METERS)`.
    local_x: f32,
    local_z: f32,
    /// Canonical world position, for point-sampling terrain.
    world_x: f32,
    world_z: f32,
    height: f32,
    rotation: f32,
    species: u8,
    /// Deterministic ordering key (source chunk, plant, seed) so landing order
    /// — and thus which candidates win spacing — is independent of parallelism.
    order: u64,
}

struct SpreadContext<'a> {
    heightmap: &'a Heightmap,
    rivers: &'a RiverField,
    registry: &'a PlantRegistry,
    biome_config: &'a BiomeConfig,
    sea_level: f32,
    born_hour: f32,
}

fn emit_chunk_candidates(
    idx: usize,
    chunks: &[Vec<Plant>],
    registry: &PlantRegistry,
    seed: u32,
) -> Vec<SpreadCandidate> {
    let n = WORLD_SIZE_CHUNKS;
    let world = WORLD_SIZE_METERS as f32;
    let cx = (idx as i32) % n;
    let cz = (idx as i32) / n;
    let canon = IVec2::new(cx, cz);

    let mut out = Vec::new();
    for (pi, plant) in chunks[idx].iter().enumerate() {
        if plant.stage != MATURE {
            continue;
        }
        let Some(species) = registry.species.get(plant.species as usize) else {
            continue;
        };
        if spread_roll(seed, canon, pi as u32) >= species.placement.spread_chance.clamp(0.0, 1.0) {
            continue;
        }

        let src_x = cx as f32 * CHUNK_SIZE_METERS + dequantize_span(plant.local_x);
        let src_z = cz as f32 * CHUNK_SIZE_METERS + dequantize_span(plant.local_z);
        let [h_lo, h_hi] = species.height_range;
        let count = spread_seed_count(seed, canon, pi as u32);
        for seed_i in 0..count {
            let sub = (pi as u32).wrapping_mul(31).wrapping_add(seed_i);
            let angle = hash_unit(seed.wrapping_add(4101), canon, sub) * std::f32::consts::TAU;
            let distance = hash_unit(seed.wrapping_add(4102), canon, sub).sqrt()
                * species.placement.spread_radius.max(0.0);
            let height = h_lo + hash_unit(seed.wrapping_add(4201), canon, sub) * (h_hi - h_lo);
            let rotation = hash_unit(seed.wrapping_add(4202), canon, sub) * std::f32::consts::TAU;

            // Wrap the seedling onto the torus, then split into canonical chunk +
            // local offset.
            let world_x = (src_x + angle.cos() * distance).rem_euclid(world);
            let world_z = (src_z + angle.sin() * distance).rem_euclid(world);
            let tcx = ((world_x / CHUNK_SIZE_METERS).floor() as i32).clamp(0, n - 1);
            let tcz = ((world_z / CHUNK_SIZE_METERS).floor() as i32).clamp(0, n - 1);
            out.push(SpreadCandidate {
                target: (tcz * n + tcx) as u32,
                local_x: world_x - tcx as f32 * CHUNK_SIZE_METERS,
                local_z: world_z - tcz as f32 * CHUNK_SIZE_METERS,
                world_x,
                world_z,
                height,
                rotation,
                species: plant.species,
                order: ((idx as u64) << 24) | ((pi as u64) << 4) | seed_i as u64,
            });
        }
    }
    out
}

fn land_chunk_candidates(
    plants: &mut Vec<Plant>,
    candidates: &mut [SpreadCandidate],
    ctx: &SpreadContext<'_>,
) -> u32 {
    if candidates.is_empty() {
        return 0;
    }
    candidates.sort_by_key(|c| c.order);

    // Spatial grid over the chunk's existing plants so each spacing query touches
    // only a 3×3 cell neighbourhood instead of every plant — without it the pass
    // is O(candidates × plants) and chokes as chunks densify.
    let mut grid = SpacingGrid::build(plants);

    let mut accepted = 0u32;
    for candidate in candidates.iter() {
        let Some(species) = ctx.registry.species.get(candidate.species as usize) else {
            continue;
        };
        let placement = &species.placement;

        // Cheap checks first; `slope_at` (12 noise samples) is left for last so a
        // candidate rejected by altitude/moisture/biome/spacing never pays for it.
        let height = ctx
            .heightmap
            .sample_height(candidate.world_x, candidate.world_z);
        if height < ctx.sea_level
            || height < placement.min_altitude
            || height > placement.max_altitude
        {
            continue;
        }
        // Reject seedlings that land in a river channel — mirrors the base-flora
        // guard so spread can't slowly recolonise the water the base pass kept clear.
        if ctx.rivers.sample(candidate.world_x, candidate.world_z).1 > MAX_PLANTABLE_WETNESS {
            continue;
        }
        let moisture = ctx
            .heightmap
            .sample_moisture(candidate.world_x, candidate.world_z);
        if moisture < placement.min_moisture || moisture > placement.max_moisture {
            continue;
        }
        let biome = classify(height, moisture, ctx.biome_config);
        if !placement.biomes.iter().any(|b| b == biome_name(biome)) {
            continue;
        }

        // Spacing against everything already in the chunk (base + earlier
        // survivors this pass). Horizontal distance — y is terrain-derived.
        let spacing = min_spacing(&species.kind);
        if grid.any_within(candidate.local_x, candidate.local_z, spacing) {
            continue;
        }

        if slope_at(ctx.heightmap, candidate.world_x, candidate.world_z) > placement.max_slope {
            continue;
        }

        grid.insert(candidate.local_x, candidate.local_z);
        plants.push(Plant {
            local_x: quantize_span(candidate.local_x),
            local_z: quantize_span(candidate.local_z),
            height: quantize_span(candidate.height),
            rotation: quantize_rotation(candidate.rotation),
            species: candidate.species,
            stage: stage_to_u8(GrowthStage::Seedling),
            born_hour: ctx.born_hour,
        });
        accepted += 1;
    }
    accepted
}

/// Uniform spatial grid of plant positions within one chunk, for O(1) spacing
/// queries. Cell size ≥ the largest spacing (8 m) so any plant within `spacing`
/// of a point lies in the point's 3×3 cell neighbourhood.
struct SpacingGrid {
    cells: Vec<Vec<(f32, f32)>>,
}

// Cell size ≥ the largest spacing (8 m) keeps the 3×3 neighbourhood correct;
// 16 m keeps the per-chunk grid small (16×16) to limit allocation churn.
const SPACING_CELL: f32 = 16.0;
const SPACING_GRID_SIDE: i32 = (CHUNK_SIZE_METERS / SPACING_CELL) as i32; // 16

impl SpacingGrid {
    fn cell_of(x: f32, z: f32) -> usize {
        let cx = ((x / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        let cz = ((z / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        (cz * SPACING_GRID_SIDE + cx) as usize
    }

    fn build(plants: &[Plant]) -> Self {
        let mut cells = vec![Vec::new(); (SPACING_GRID_SIDE * SPACING_GRID_SIDE) as usize];
        for plant in plants {
            let x = dequantize_span(plant.local_x);
            let z = dequantize_span(plant.local_z);
            cells[Self::cell_of(x, z)].push((x, z));
        }
        Self { cells }
    }

    fn insert(&mut self, x: f32, z: f32) {
        self.cells[Self::cell_of(x, z)].push((x, z));
    }

    fn any_within(&self, x: f32, z: f32, spacing: f32) -> bool {
        let spacing_sq = spacing * spacing;
        let cx = ((x / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        let cz = ((z / SPACING_CELL) as i32).clamp(0, SPACING_GRID_SIDE - 1);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let nx = cx + dx;
                let nz = cz + dz;
                if nx < 0 || nz < 0 || nx >= SPACING_GRID_SIDE || nz >= SPACING_GRID_SIDE {
                    continue;
                }
                for &(px, pz) in &self.cells[(nz * SPACING_GRID_SIDE + nx) as usize] {
                    let ddx = px - x;
                    let ddz = pz - z;
                    if ddx * ddx + ddz * ddz < spacing_sq {
                        return true;
                    }
                }
            }
        }
        false
    }
}

fn spread_roll(seed: u32, canon: IVec2, plant_index: u32) -> f32 {
    hash_unit(seed.wrapping_add(4001), canon, plant_index)
}

fn spread_seed_count(seed: u32, canon: IVec2, plant_index: u32) -> u32 {
    1 + (hash_unit(seed.wrapping_add(4002), canon, plant_index) * 2.0).floor() as u32
}

fn hash_unit(seed: u32, canon: IVec2, sub: u32) -> f32 {
    hash_to_unit_float(hash4(seed, canon.x as u32, canon.y as u32, sub))
}

fn slope_at(heightmap: &Heightmap, x: f32, z: f32) -> f32 {
    let d = 1.75;
    let hx0 = heightmap.sample_height(x - d, z);
    let hx1 = heightmap.sample_height(x + d, z);
    let hz0 = heightmap.sample_height(x, z - d);
    let hz1 = heightmap.sample_height(x, z + d);
    let dx = (hx1 - hx0) / (2.0 * d);
    let dz = (hz1 - hz0) / (2.0 * d);
    (dx * dx + dz * dz).sqrt()
}

fn min_spacing(kind: &str) -> f32 {
    if kind == "shrub" {
        3.0
    } else {
        8.0
    }
}

fn biome_name(biome: Biome) -> &'static str {
    match biome {
        Biome::Forest => "Forest",
        Biome::Grassland => "Grassland",
        Biome::Desert => "Desert",
        Biome::Rock => "Rock",
        Biome::Snow => "Snow",
    }
}

const BIOME_SLOTS: usize = 5;

/// Order biomes are reported in. Output order, independent of enum discriminant.
const BIOME_ORDER: [Biome; BIOME_SLOTS] = [
    Biome::Forest,
    Biome::Grassland,
    Biome::Desert,
    Biome::Rock,
    Biome::Snow,
];

/// Stable per-biome slot for the fill tallies. An exhaustive match (rather than
/// `biome as usize`) so adding a `Biome` variant is a compile error here, not an
/// out-of-bounds index at runtime.
fn biome_slot(biome: Biome) -> usize {
    match biome {
        Biome::Forest => 0,
        Biome::Grassland => 1,
        Biome::Desert => 2,
        Biome::Rock => 3,
        Biome::Snow => 4,
    }
}

/// Inverse of [`biome_slot`], for decoding the base-world snapshot. Unknown codes
/// fall back to `Snow` rather than panicking on a corrupt byte.
fn biome_from_slot(slot: u8) -> Biome {
    match slot {
        0 => Biome::Forest,
        1 => Biome::Grassland,
        2 => Biome::Desert,
        3 => Biome::Rock,
        _ => Biome::Snow,
    }
}

// ---------------------------------------------------------------------------
// Spread persistence — compact little-endian blob of the per-chunk spread suffix
// ---------------------------------------------------------------------------

const SPREAD_MAGIC: u32 = 0x504c_4e54; // "PLNT"
const SPREAD_VERSION: u32 = 1;
/// Serialized size of one plant: u16×4 + u8×2 + f32.
const PLANT_BYTES: usize = 14;

// ---------------------------------------------------------------------------
// Base-world snapshot — full per-chunk base flora + biome/map, a cache of the
// expensive `generate_base` pass keyed by the generation inputs.
//
// v2 stores plants as three Brotli-compressed columns (meta / position-deltas /
// attributes) instead of a flat 14-byte record. Combined with reduced field
// precision and Morton-delta positions this takes the default world from
// ~190 MiB to ~31 MiB. Base plants are snapped to this grid at generation time
// (`Plant::pack`) so a downloaded snapshot is bit-identical to a local one.
// ---------------------------------------------------------------------------

const BASE_MAGIC: u32 = 0x5742_4153; // "WBAS"
const BASE_VERSION: u32 = 2;

/// Bits of position precision stored per axis, over the 256 m chunk: 10 bits →
/// a 0.25 m grid. `POS_SHIFT` is how far that sits below the resident `u16`
/// (`local_x`/`local_z`) span, so a stored code is `local >> POS_SHIFT`.
const POS_BITS: u32 = 10;
const POS_SHIFT: u16 = 16 - POS_BITS as u16; // 6
/// Number of distinct Morton cells on the position grid (`2^(2*POS_BITS)`), the
/// exclusive upper bound for a stored position code and a hard ceiling on plants
/// per chunk during load.
const MORTON_CELLS: usize = 1 << (2 * POS_BITS);

/// Plant height is stored as a `u8` over `[0, HEIGHT_RANGE_M)` → a 0.125 m step.
/// The tallest default species tops out at 25 m, so 32 m leaves headroom; a
/// taller plant would clamp (cosmetic only — height never feeds spread).
const HEIGHT_RANGE_M: f32 = 32.0;

/// Brotli window (`lgwin`, 10–24) for the snapshot sections; 24 (16 MiB) lets the
/// compressor reach back across a whole column. Quality is chosen per call —
/// see [`PlantWorld::DOWNLOAD_QUALITY`] / [`PlantWorld::LOCAL_QUALITY`].
const BASE_BROTLI_LGWIN: u32 = 24;

/// Cap on a single decompressed section, to bound allocation from a corrupt or
/// hostile header before the bytes are trusted.
const BASE_SECTION_MAX_BYTES: usize = 512 * 1024 * 1024;

/// Mask a quantized position down to the `POS_BITS` storage grid (clears the low
/// `POS_SHIFT` bits) so generation and snapshot round-trips agree exactly.
fn snap_pos(v: u16) -> u16 {
    v & !((1u16 << POS_SHIFT) - 1)
}

/// Quantize a height in metres to the 8-bit storage range.
fn pack_height(height_m: f32) -> u8 {
    (height_m / HEIGHT_RANGE_M * 256.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// Inverse of [`pack_height`], back into the resident `u16` height span so
/// [`dequantize_span`] reproduces the stored metres.
fn unpack_height(h8: u8) -> u16 {
    quantize_span(h8 as f32 / 256.0 * HEIGHT_RANGE_M)
}

/// Quantize a rotation in radians to 8 bits over `[0, TAU)` (wraps at TAU).
fn pack_rotation(radians: f32) -> u8 {
    ((radians.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 256.0).round() as i64
        & 0xFF) as u8
}

/// Interleave the low `POS_BITS` of `x` and `z` into a Morton (Z-order) code.
/// Sorting a chunk by this keeps spatially-near plants adjacent, so their
/// delta-coded positions stay small. A plain bit loop — only run once per plant
/// at generation/serialization, never on a hot path.
fn morton10(x: u16, z: u16) -> u32 {
    let mut code = 0u32;
    for i in 0..POS_BITS {
        code |= ((x as u32 >> i) & 1) << (2 * i);
        code |= ((z as u32 >> i) & 1) << (2 * i + 1);
    }
    code
}

/// Inverse of [`morton10`].
fn unmorton10(code: u32) -> (u16, u16) {
    let mut x = 0u16;
    let mut z = 0u16;
    for i in 0..POS_BITS {
        x |= (((code >> (2 * i)) & 1) as u16) << i;
        z |= (((code >> (2 * i + 1)) & 1) as u16) << i;
    }
    (x, z)
}

/// Sort a chunk's plants into canonical Morton order (stable, so equal cells keep
/// generation order). This both shrinks the snapshot and pins each plant's index
/// within its chunk — which spread RNG reads — so every client agrees.
fn sort_chunk_canonical(plants: &mut [Plant]) {
    plants.sort_by_key(|p| morton10(p.local_x >> POS_SHIFT, p.local_z >> POS_SHIFT));
}

/// LEB128 unsigned varint, little-endian groups of 7 bits. Morton deltas are
/// mostly tiny, so this is where the position stream shrinks before compression.
fn write_varint(buf: &mut Vec<u8>, mut v: u32) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn read_varint(cur: &mut Cursor<'_>) -> anyhow::Result<u32> {
    let mut result = 0u32;
    let mut shift = 0u32;
    loop {
        if shift >= 32 {
            anyhow::bail!("varint too long");
        }
        let byte = cur.read_u8()?;
        result |= ((byte & 0x7f) as u32) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}

/// Brotli-compress a section body. Writing to a `Vec` is infallible, so this
/// can't error.
fn brotli_compress(data: &[u8], quality: u32) -> Vec<u8> {
    use std::io::Write;
    let mut out = Vec::new();
    {
        let mut writer = brotli::CompressorWriter::new(&mut out, 4096, quality, BASE_BROTLI_LGWIN);
        writer
            .write_all(data)
            .expect("brotli compression into a Vec is infallible");
    }
    out
}

/// Brotli-decompress, refusing anything that expands past `max_len` so a crafted
/// stream can't blow up memory.
fn brotli_decompress(data: &[u8], max_len: usize) -> anyhow::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    brotli::Decompressor::new(data, 4096)
        .take(max_len as u64 + 1)
        .read_to_end(&mut out)?;
    if out.len() > max_len {
        anyhow::bail!("decompressed section exceeds declared length");
    }
    Ok(out)
}

/// Append a length-framed, Brotli-compressed section at the given `quality`:
/// `uncompressed_len(u32)`, `compressed_len(u32)`, then the compressed bytes.
fn write_section(buf: &mut Vec<u8>, data: &[u8], quality: u32) {
    let comp = brotli_compress(data, quality);
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&(comp.len() as u32).to_le_bytes());
    buf.extend_from_slice(&comp);
}

/// Read and decompress one [`write_section`] frame, validating both lengths.
fn read_section(cur: &mut Cursor<'_>) -> anyhow::Result<Vec<u8>> {
    let uncomp_len = cur.read_u32()? as usize;
    let comp_len = cur.read_u32()? as usize;
    if uncomp_len > BASE_SECTION_MAX_BYTES {
        anyhow::bail!("section length {uncomp_len} exceeds {BASE_SECTION_MAX_BYTES}");
    }
    if comp_len > cur.remaining() {
        anyhow::bail!("compressed section {comp_len} exceeds remaining data");
    }
    let comp = cur.take(comp_len)?;
    let out = brotli_decompress(comp, uncomp_len)?;
    if out.len() != uncomp_len {
        anyhow::bail!(
            "section length mismatch: decoded {} want {uncomp_len}",
            out.len()
        );
    }
    Ok(out)
}

fn write_plant(buf: &mut Vec<u8>, p: &Plant) {
    buf.extend_from_slice(&p.local_x.to_le_bytes());
    buf.extend_from_slice(&p.local_z.to_le_bytes());
    buf.extend_from_slice(&p.height.to_le_bytes());
    buf.extend_from_slice(&p.rotation.to_le_bytes());
    buf.push(p.species);
    buf.push(p.stage);
    buf.extend_from_slice(&p.born_hour.to_le_bytes());
}

fn read_plant(cur: &mut Cursor<'_>) -> anyhow::Result<Plant> {
    Ok(Plant {
        local_x: cur.read_u16()?,
        local_z: cur.read_u16()?,
        height: cur.read_u16()?,
        rotation: cur.read_u16()?,
        species: cur.read_u8()?,
        stage: cur.read_u8()?,
        born_hour: cur.read_f32()?,
    })
}

/// Minimal bounds-checked little-endian reader over a byte slice.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn take(&mut self, n: usize) -> anyhow::Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.buf.len());
        let Some(end) = end else {
            anyhow::bail!("unexpected end of data");
        };
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, n: usize) -> anyhow::Result<()> {
        self.take(n)?;
        Ok(())
    }

    fn read_u8(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn read_u16(&mut self) -> anyhow::Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn read_u32(&mut self) -> anyhow::Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u64(&mut self) -> anyhow::Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_f32(&mut self) -> anyhow::Result<f32> {
        let b = self.take(4)?;
        Ok(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// Analytic growth stage for a packed plant at `total_hours`.
fn stage_for(plant: &Plant, total_hours: f64, registry: &PlantRegistry) -> GrowthStage {
    let Some(species) = registry.species.get(plant.species as usize) else {
        return stage_from_u8(plant.stage);
    };
    let age = (total_hours - plant.born_hour as f64).max(0.0);
    let young_at = species.placement.seedling_hours.max(0.0) as f64;
    let mature_at = young_at + species.placement.young_hours.max(0.0) as f64;
    if age >= mature_at {
        GrowthStage::Mature
    } else if age >= young_at {
        GrowthStage::Young
    } else {
        GrowthStage::Seedling
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_core::herbarium::Herbarium;
    use crate::world_core::storage::Storage as _;

    fn registry() -> Arc<PlantRegistry> {
        Arc::new(PlantRegistry::from_herbarium(&Herbarium::default_seeded()))
    }

    // A small PlantWorld with the given per-chunk plant lists; terrain/rules come
    // from the default config so spread validation samples the real heightmap.
    fn test_world(chunks: Vec<Vec<Plant>>, reg: Arc<PlantRegistry>) -> PlantWorld {
        let config = GameConfig::default();
        let population = chunks.iter().map(Vec::len).sum();
        let populated_chunks = chunks.iter().filter(|c| !c.is_empty()).count();
        let immature = chunks
            .iter()
            .map(|c| c.iter().filter(|p| p.stage != MATURE).count() as u32)
            .collect();
        let saturated = vec![false; chunks.len()];
        let base_count = chunks.iter().map(|c| c.len() as u32).collect();
        let biome = vec![Biome::Forest; chunks.len()];
        let cell_bytes = vec![0u8; chunks.len()];
        PlantWorld {
            chunks,
            immature,
            saturated,
            base_count,
            biome,
            cell_bytes,
            registry: reg,
            heightmap: Heightmap::new(7, config.heightmap.clone()),
            rivers: Arc::new(RiverField::empty()),
            biome_config: config.biome.clone(),
            sea_level: config.sea_level,
            seed: 7,
            population,
            populated_chunks,
            last_spread_added: 0,
            biome_fill: Vec::new(),
        }
    }

    // In-memory storage with binary support, for persistence round-trips.
    #[derive(Default)]
    struct MemBytes {
        bytes: std::cell::RefCell<std::collections::HashMap<String, Vec<u8>>>,
    }
    impl crate::world_core::storage::Storage for MemBytes {
        fn load(&self, _key: &str) -> Option<String> {
            None
        }
        fn save(&self, _key: &str, _data: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn load_bytes(&self, key: &str) -> Option<Vec<u8>> {
            self.bytes.borrow().get(key).cloned()
        }
        fn save_bytes(&self, key: &str, data: &[u8]) -> anyhow::Result<()> {
            self.bytes
                .borrow_mut()
                .insert(key.to_string(), data.to_vec());
            Ok(())
        }
    }

    fn seedling(local_x: f32, born_hour: f32) -> Plant {
        Plant {
            local_x: quantize_span(local_x),
            local_z: quantize_span(50.0),
            height: quantize_span(4.0),
            rotation: quantize_rotation(1.0),
            species: 0,
            stage: stage_to_u8(GrowthStage::Seedling),
            born_hour,
        }
    }

    // A mature base plant already snapped to the base-snapshot storage grid,
    // exactly as `Plant::pack` produces — so a base-snapshot round-trip is
    // bit-identical and `assert_eq!` on whole chunks holds.
    fn grid_plant(local_x: f32, local_z: f32, species: u8) -> Plant {
        let h8 = pack_height(4.0);
        let r8 = pack_rotation(1.0);
        Plant {
            local_x: snap_pos(quantize_span(local_x)),
            local_z: snap_pos(quantize_span(local_z)),
            height: unpack_height(h8),
            rotation: (r8 as u16) << 8,
            species,
            stage: MATURE,
            born_hour: 0.0,
        }
    }

    #[test]
    fn spread_state_round_trips_through_save_and_load() {
        let reg = registry();
        // A world whose chunk 1 has one base plant plus two spread seedlings.
        let base = Plant {
            stage: MATURE,
            ..seedling(10.0, 0.0)
        };
        let mut world = test_world(vec![Vec::new(), vec![base], Vec::new()], Arc::clone(&reg));
        // base_count was set to current lengths, so anything appended now is spread.
        world.chunks[1].push(seedling(20.0, 24.0));
        world.chunks[1].push(seedling(30.0, 24.0));
        world.immature[1] = 2;
        world.population += 2;

        let storage = MemBytes::default();
        world.save_spread(&storage).unwrap();

        // Reload: a fresh world with only the base, then apply the saved spread.
        let mut reloaded = test_world(vec![Vec::new(), vec![base], Vec::new()], reg);
        let restored = reloaded.apply_saved_spread(&storage);

        assert_eq!(restored, 2);
        assert_eq!(reloaded.population, world.population);
        assert_eq!(reloaded.chunks[1].len(), 3);
        assert_eq!(reloaded.immature[1], 2);
        // The spread suffix is byte-identical.
        assert_eq!(&reloaded.chunks[1][1..], &world.chunks[1][1..]);
        // Saturation is seeded from maturity so a settled resume is cheap: the
        // all-mature empty chunks are saturated, the chunk with seedlings is not.
        assert!(reloaded.saturated[0]);
        assert!(!reloaded.saturated[1]);
        assert!(reloaded.saturated[2]);
    }

    #[test]
    fn base_snapshot_round_trips_and_rebuilds_indices() {
        let reg = registry();
        // A full-size world (the snapshot format is fixed to WORLD_SIZE_CHUNKS²)
        // with a few populated chunks. Base flora is always mature and snapped to
        // the storage grid, so plant lists round-trip bit-identically once each
        // chunk is in canonical Morton order.
        let total = (WORLD_SIZE_CHUNKS as usize) * (WORLD_SIZE_CHUNKS as usize);
        let mut chunks = vec![Vec::new(); total];
        chunks[5].push(grid_plant(10.0, 50.0, 0));
        chunks[5].push(grid_plant(20.0, 80.0, 3));
        chunks[total - 1].push(grid_plant(10.0, 50.0, 1));
        for c in &mut chunks {
            sort_chunk_canonical(c);
        }
        let world = test_world(chunks, Arc::clone(&reg));

        let config = GameConfig::default();
        let gen_key = 0xABCD_1234_5678_9A00u64;

        // Brotli decode is quality-agnostic, so a fast local cache (q1) and the
        // shipped download (q10) share the v2 layout and round-trip identically.
        for quality in [PlantWorld::LOCAL_QUALITY, PlantWorld::DOWNLOAD_QUALITY] {
            let bytes = world.serialize_base(gen_key, quality);
            // The blob is the v2 compressed format, not a flat plant dump.
            assert_eq!(&bytes[0..4], &BASE_MAGIC.to_le_bytes());
            assert_eq!(&bytes[4..8], &BASE_VERSION.to_le_bytes());

            let restored = PlantWorld::from_base_snapshot(
                &bytes,
                gen_key,
                world.seed,
                &config,
                Arc::clone(&reg),
                Arc::new(RiverField::empty()),
            )
            .expect("a matching snapshot loads");
            assert_eq!(restored.population(), world.population());
            assert_eq!(restored.populated_chunks(), world.populated_chunks());
            // Plant lists round-trip byte-for-byte; derived indices are rebuilt.
            // Base plants are all mature, so nothing is immature after a load.
            assert_eq!(restored.chunks[5], world.chunks[5]);
            assert_eq!(restored.base_count[5], 2);
            assert_eq!(restored.immature[5], 0);
            assert_eq!(restored.chunks[total - 1], world.chunks[total - 1]);

            // A snapshot is rejected (falls back to generation) when the
            // generation key or the seed does not match.
            assert!(
                PlantWorld::from_base_snapshot(
                    &bytes,
                    gen_key ^ 1,
                    world.seed,
                    &config,
                    registry(),
                    Arc::new(RiverField::empty()),
                )
                .is_none(),
                "a changed generation key must invalidate the cache"
            );
            assert!(
                PlantWorld::from_base_snapshot(
                    &bytes,
                    gen_key,
                    world.seed + 1,
                    &config,
                    registry(),
                    Arc::new(RiverField::empty()),
                )
                .is_none(),
                "a different seed must invalidate the cache"
            );
        }
    }

    #[test]
    fn apply_saved_spread_rejects_a_corrupt_plant_count() {
        // A blob whose declared plant count far exceeds its bytes must be rejected
        // (not reserve a huge allocation) — header + one chunk claiming 1e9 plants.
        let mut buf = Vec::new();
        buf.extend_from_slice(&SPREAD_MAGIC.to_le_bytes());
        buf.extend_from_slice(&SPREAD_VERSION.to_le_bytes());
        buf.extend_from_slice(&7u32.to_le_bytes()); // seed (matches test_world)
        buf.extend_from_slice(&1u32.to_le_bytes()); // one chunk
        buf.extend_from_slice(&0u32.to_le_bytes()); // chunk idx 0
        buf.extend_from_slice(&1_000_000_000u32.to_le_bytes()); // bogus count
        let storage = MemBytes::default();
        storage.save_bytes("plants", &buf).unwrap();

        let mut world = test_world(vec![vec![]], registry());
        assert_eq!(world.apply_saved_spread(&storage), 0);
        assert!(world.chunks[0].is_empty());
    }

    #[test]
    fn apply_saved_spread_rejects_a_mismatched_seed() {
        let reg = registry();
        let mut world = test_world(vec![vec![], vec![seedling(10.0, 0.0)]], Arc::clone(&reg));
        world.chunks[1].push(seedling(20.0, 24.0));
        world.immature[1] = 2;
        let storage = MemBytes::default();
        world.save_spread(&storage).unwrap();

        // A world with a different seed must ignore the save rather than corrupt.
        let mut other = test_world(vec![vec![], vec![seedling(10.0, 0.0)]], reg);
        other.seed = 999;
        assert_eq!(other.apply_saved_spread(&storage), 0);
        assert_eq!(other.chunks[1].len(), 1);
    }

    #[test]
    fn packed_plant_is_sixteen_bytes() {
        assert_eq!(std::mem::size_of::<Plant>(), 16);
    }

    #[test]
    fn pack_round_trips_within_quantization_tolerance() {
        let origin_x = 5.0 * CHUNK_SIZE_METERS;
        let origin_z = 7.0 * CHUNK_SIZE_METERS;
        let original = PlantInstance {
            position: Vec3::new(origin_x + 123.4, 88.0, origin_z + 200.1),
            rotation: 2.0,
            height: 12.5,
            species_index: 3,
            growth_stage: GrowthStage::Mature,
        };
        let packed = Plant::pack(&original, origin_x, origin_z);

        // Reconstruct against flat terrain so y is well-defined.
        let total = crate::world_core::chunk::CHUNK_GRID_RESOLUTION
            * crate::world_core::chunk::CHUNK_GRID_RESOLUTION;
        let terrain = ChunkTerrain {
            heights: vec![88.0; total],
            moisture: vec![0.5; total],
            river: vec![0.0; total],
            min_height: 88.0,
            max_height: 88.0,
            has_water: false,
        };
        let back = packed.to_instance(IVec2::new(5, 7), &terrain);

        // Base plants are snapped to the storage grid in `pack`: 0.25 m per axis
        // (`POS_BITS`), 0.125 m height, ~1.4° rotation.
        assert!((back.position.x - original.position.x).abs() < 0.25);
        assert!((back.position.z - original.position.z).abs() < 0.25);
        assert!((back.position.y - 88.0).abs() < 1e-3);
        assert!((back.height - original.height).abs() < 0.13);
        assert!((back.rotation - original.rotation).abs() < 0.03);
        assert_eq!(back.species_index, original.species_index);
        assert_eq!(back.growth_stage, GrowthStage::Mature);
    }

    #[test]
    fn instances_rebase_to_the_raw_chunk_one_lap_away() {
        // A canonical chunk rendered a full world lap east lands one lap further
        // along x with its local offset preserved.
        let total = crate::world_core::chunk::CHUNK_GRID_RESOLUTION
            * crate::world_core::chunk::CHUNK_GRID_RESOLUTION;
        let terrain = ChunkTerrain {
            heights: vec![50.0; total],
            moisture: vec![0.5; total],
            river: vec![0.0; total],
            min_height: 50.0,
            max_height: 50.0,
            has_water: false,
        };
        let plant = Plant {
            local_x: quantize_span(40.0),
            local_z: quantize_span(60.0),
            height: quantize_span(10.0),
            rotation: 0,
            species: 0,
            stage: MATURE,
            born_hour: 0.0,
        };
        let canon = IVec2::new(3, 4);
        let raw = IVec2::new(3 + WORLD_SIZE_CHUNKS, 4);

        let here = plant.to_instance(canon, &terrain);
        let lap = plant.to_instance(raw, &terrain);

        let lap_meters = WORLD_SIZE_CHUNKS as f32 * CHUNK_SIZE_METERS;
        assert!((lap.position.x - (here.position.x + lap_meters)).abs() < 0.01);
        assert!((lap.position.z - here.position.z).abs() < 0.01);
        assert!((lap.position.y - here.position.y).abs() < 1e-3);
    }

    #[test]
    fn neighbourhood_saturation_requires_self_and_all_eight_neighbours() {
        let n = WORLD_SIZE_CHUNKS;
        let cells = (n * n) as usize;
        let saturate = |sat: &mut [bool], cx: i32, cz: i32| {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let nx = (cx + dx).rem_euclid(n);
                    let nz = (cz + dz).rem_euclid(n);
                    sat[(nz * n + nx) as usize] = true;
                }
            }
        };

        // Interior chunk with its full 3×3 block saturated.
        let mut sat = vec![false; cells];
        saturate(&mut sat, 5, 5);
        let idx = (5 * n + 5) as usize;
        assert!(neighbourhood_saturated(&sat, idx));

        // Clearing any one neighbour breaks it.
        sat[(6 * n + 5) as usize] = false;
        assert!(!neighbourhood_saturated(&sat, idx));

        // The centre itself must be saturated too.
        sat[(6 * n + 5) as usize] = true;
        sat[idx] = false;
        assert!(!neighbourhood_saturated(&sat, idx));

        // Corner chunk (0,0): neighbours wrap around the torus to row/col n-1.
        let mut wrap = vec![false; cells];
        saturate(&mut wrap, 0, 0);
        assert!(neighbourhood_saturated(&wrap, 0));
        wrap[((n - 1) * n + (n - 1)) as usize] = false; // the (-1,-1) diagonal
        assert!(!neighbourhood_saturated(&wrap, 0));
    }

    #[test]
    fn tick_growth_promotes_immature_plants_to_mature() {
        let reg = registry();
        let species = &reg.species[0];
        let mature_at = (species.placement.seedling_hours + species.placement.young_hours) as f64;

        let mut world = test_world(
            vec![vec![Plant {
                local_x: 0,
                local_z: 0,
                height: quantize_span(5.0),
                rotation: 0,
                species: 0,
                stage: stage_to_u8(GrowthStage::Seedling),
                born_hour: 0.0,
            }]],
            Arc::clone(&reg),
        );

        // Before any growth time, it stays a seedling.
        assert!(!world.tick_growth(0.0));
        assert_eq!(world.chunks[0][0].stage, stage_to_u8(GrowthStage::Seedling));
        assert_eq!(world.immature[0], 1);

        // Well past maturity, it becomes mature and the chunk drops to zero
        // immature — so later ticks skip it.
        assert!(world.tick_growth(mature_at + 1.0));
        assert_eq!(world.chunks[0][0].stage, MATURE);
        assert_eq!(world.immature[0], 0);
    }

    // Find a canonical chunk + local position suitable for `species_idx` so the
    // spread-landing test exercises the real terrain validation deterministically.
    fn find_suitable_spot(world: &PlantWorld, species_idx: usize) -> (usize, f32, f32) {
        let placement = &world.registry.species[species_idx].placement;
        let n = WORLD_SIZE_CHUNKS;
        for cz in 0..n {
            for cx in 0..n {
                // Sample the chunk centre.
                let wx = cx as f32 * CHUNK_SIZE_METERS + 128.0;
                let wz = cz as f32 * CHUNK_SIZE_METERS + 128.0;
                let height = world.heightmap.sample_height(wx, wz);
                let moisture = world.heightmap.sample_moisture(wx, wz);
                if height < world.sea_level
                    || height < placement.min_altitude
                    || height > placement.max_altitude
                    || moisture < placement.min_moisture
                    || moisture > placement.max_moisture
                    || slope_at(&world.heightmap, wx, wz) > placement.max_slope
                {
                    continue;
                }
                let biome = classify(height, moisture, &world.biome_config);
                if placement.biomes.iter().any(|b| b == biome_name(biome)) {
                    return ((cz * n + cx) as usize, 128.0, 128.0);
                }
            }
        }
        panic!("no suitable spot found for species {species_idx}");
    }

    #[test]
    fn spread_landing_enforces_species_spacing() {
        // Many candidates crowded onto one suitable spot: spacing admits only one.
        let reg = registry();
        let world = test_world(vec![Vec::new(); 4], Arc::clone(&reg));
        let (chunk_idx, lx, lz) = find_suitable_spot(&world, 0);
        let cx = (chunk_idx as i32) % WORLD_SIZE_CHUNKS;
        let cz = (chunk_idx as i32) / WORLD_SIZE_CHUNKS;
        let wx = cx as f32 * CHUNK_SIZE_METERS + lx;
        let wz = cz as f32 * CHUNK_SIZE_METERS + lz;

        let ctx = SpreadContext {
            heightmap: &world.heightmap,
            rivers: &world.rivers,
            registry: &reg,
            biome_config: &world.biome_config,
            sea_level: world.sea_level,
            born_hour: 0.0,
        };
        let mk = |order: u64| SpreadCandidate {
            target: chunk_idx as u32,
            local_x: lx,
            local_z: lz,
            world_x: wx,
            world_z: wz,
            height: 6.0,
            rotation: 0.0,
            species: 0,
            order,
        };
        let mut plants = Vec::new();
        let mut crowd = vec![mk(0), mk(1), mk(2), mk(3), mk(4)];
        let accepted = land_chunk_candidates(&mut plants, &mut crowd, &ctx);
        assert_eq!(accepted, 1, "spacing must reject coincident candidates");
        assert_eq!(plants.len(), 1);

        // A second pass at the very same spot adds nothing — the gate is stable,
        // which is what makes the global spread terminate.
        let mut again = vec![mk(5)];
        assert_eq!(land_chunk_candidates(&mut plants, &mut again, &ctx), 0);
        assert_eq!(plants.len(), 1);
    }

    #[test]
    fn spread_emits_seedlings_for_mature_plants() {
        // A mature plant whose spread roll succeeds emits at least one candidate
        // anchored near it; immature plants emit none.
        let reg = registry();
        let n = WORLD_SIZE_CHUNKS;
        // Find a chunk+plant_index whose spread roll fires for species 0.
        let species0 = &reg.species[0];
        let chance = species0.placement.spread_chance.clamp(0.0, 1.0);
        let canon = (0..n)
            .flat_map(|z| (0..n).map(move |x| IVec2::new(x, z)))
            .find(|c| spread_roll(7, *c, 0) < chance)
            .expect("a chunk whose plant 0 spreads");
        let idx = (canon.y * n + canon.x) as usize;
        let mut chunks = vec![Vec::new(); (n * n) as usize];
        chunks[idx].push(Plant {
            local_x: quantize_span(128.0),
            local_z: quantize_span(128.0),
            height: quantize_span(10.0),
            rotation: 0,
            species: 0,
            stage: MATURE,
            born_hour: 0.0,
        });
        let mature = emit_chunk_candidates(idx, &chunks, &reg, 7);
        assert!(!mature.is_empty(), "a firing mature plant emits candidates");
        for c in &mature {
            // Candidate world position is within spread_radius of the source.
            let dx = c.world_x - (canon.x as f32 * CHUNK_SIZE_METERS + 128.0);
            let dz = c.world_z - (canon.y as f32 * CHUNK_SIZE_METERS + 128.0);
            assert!((dx * dx + dz * dz).sqrt() <= species0.placement.spread_radius + 0.01);
        }

        // The same plant as a seedling emits nothing.
        chunks[idx][0].stage = stage_to_u8(GrowthStage::Seedling);
        assert!(emit_chunk_candidates(idx, &chunks, &reg, 7).is_empty());
    }
}
