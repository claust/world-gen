//! Lock-free progress handle for async world generation.
//!
//! `generate_base` runs on a worker thread and writes its per-chunk progress
//! here; the loading UI reads it every frame to paint the "world being born"
//! map. The hot path is a single relaxed atomic store + counter bump per chunk,
//! so there is no mutex between the worker and the UI.

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::world_core::biome::Biome;
use crate::world_core::chunk::WORLD_SIZE_CHUNKS;

/// One cell per canonical chunk (`WORLD_SIZE_CHUNKS²`).
pub const GEN_CELL_COUNT: usize = (WORLD_SIZE_CHUNKS as usize) * (WORLD_SIZE_CHUNKS as usize);

/// Cell state byte. `0` means "not yet generated"; any other value is
/// `biome/water code + 1` so a freshly-zeroed buffer reads as empty.
pub const CELL_EMPTY: u8 = 0;

// Cell codes (stored as `code + 1` so 0 stays "empty").
const CODE_FOREST: u8 = 0;
const CODE_GRASSLAND: u8 = 1;
const CODE_DESERT: u8 = 2;
const CODE_ROCK: u8 = 3;
const CODE_SNOW: u8 = 4;
const CODE_WATER: u8 = 5;

/// Map a finished chunk's classified biome (plus a water override) to its stored
/// cell byte. `is_water` wins over the biome so submerged chunks paint blue,
/// matching the minimap.
pub fn cell_byte(biome: Biome, is_water: bool) -> u8 {
    let code = if is_water {
        CODE_WATER
    } else {
        match biome {
            Biome::Forest => CODE_FOREST,
            Biome::Grassland => CODE_GRASSLAND,
            Biome::Desert => CODE_DESERT,
            Biome::Rock => CODE_ROCK,
            Biome::Snow => CODE_SNOW,
        }
    };
    code + 1
}

/// RGBA color for a stored cell byte. `CELL_EMPTY` is fully transparent so the
/// unfilled map shows the background through it; the palette mirrors
/// `minimap_colors::biome_color_rgba`.
pub fn cell_color(byte: u8) -> [u8; 4] {
    match byte {
        b if b == CODE_FOREST + 1 => [46, 97, 46, 255], // dark green
        b if b == CODE_GRASSLAND + 1 => [77, 122, 51, 255], // green
        b if b == CODE_DESERT + 1 => [174, 148, 87, 255], // tan
        b if b == CODE_ROCK + 1 => [107, 107, 110, 255], // gray
        b if b == CODE_SNOW + 1 => [224, 230, 237, 255], // near-white
        b if b == CODE_WATER + 1 => [38, 77, 140, 255], // blue
        _ => [0, 0, 0, 0],                              // empty
    }
}

/// Shared between the generation worker and the loading UI.
pub struct GenerationProgress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Per-chunk cell state, row-major (`idx = cz * WORLD_SIZE_CHUNKS + cx`).
    cells: Box<[AtomicU8]>,
}

impl GenerationProgress {
    pub fn new() -> Self {
        let mut cells = Vec::with_capacity(GEN_CELL_COUNT);
        cells.resize_with(GEN_CELL_COUNT, || AtomicU8::new(CELL_EMPTY));
        Self {
            done: AtomicUsize::new(0),
            total: AtomicUsize::new(GEN_CELL_COUNT),
            cells: cells.into_boxed_slice(),
        }
    }

    /// Record a finished chunk: store its cell byte and bump the done counter.
    /// Called once per chunk from the (parallel) generation closure.
    pub fn record(&self, idx: usize, byte: u8) {
        if let Some(cell) = self.cells.get(idx) {
            cell.store(byte, Ordering::Relaxed);
        }
        self.done.fetch_add(1, Ordering::Relaxed);
    }

    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Completed fraction in `[0, 1]`.
    pub fn fraction(&self) -> f32 {
        let total = self.total().max(1);
        (self.done() as f32 / total as f32).clamp(0.0, 1.0)
    }

    /// Read one cell byte (UI side).
    pub fn cell(&self, idx: usize) -> u8 {
        self.cells
            .get(idx)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(CELL_EMPTY)
    }
}

impl Default for GenerationProgress {
    fn default() -> Self {
        Self::new()
    }
}
