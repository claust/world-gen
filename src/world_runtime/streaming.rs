use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{IVec2, Vec3};

use super::chunk_loader::{ChunkLoader, PlatformLoader};
use super::plant_world::PlantWorld;
use crate::world_core::chunk::{ChunkData, CHUNK_SIZE_METERS};
use crate::world_core::config::GameConfig;
use crate::world_core::evolution::EvolutionOverlayMode;
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::rivers::RiverField;

pub struct StreamingStats {
    pub loaded_chunks: usize,
    pub pending_chunks: usize,
    pub center_chunk: IVec2,
}

// ---------------------------------------------------------------------------
// StreamingWorld — unified orchestration, delegates loading to PlatformLoader
// ---------------------------------------------------------------------------

pub struct StreamingWorld {
    seed: u32,
    load_radius: i32,
    loaded: HashMap<IVec2, ChunkData>,
    center_chunk: IVec2,
    loader: PlatformLoader,
    thread_count: usize,
    config: Arc<GameConfig>,
    registry: Arc<PlantRegistry>,
    rivers: Arc<RiverField>,
}

impl StreamingWorld {
    pub fn new(
        seed: u32,
        load_radius: i32,
        threads: usize,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
        rivers: Arc<RiverField>,
    ) -> anyhow::Result<Self> {
        let loader = PlatformLoader::new_loader(
            seed,
            threads,
            Arc::clone(&config),
            Arc::clone(&registry),
            Arc::clone(&rivers),
        )?;

        // No synchronous chunk generation in this constructor — all chunks (including
        // the center) are dispatched via update(); native loaders use background
        // threads, while the wasm32 loader runs synchronously on the main thread.
        Ok(Self {
            seed,
            load_radius,
            loaded: HashMap::new(),
            center_chunk: IVec2::ZERO,
            loader,
            thread_count: threads,
            config,
            registry,
            rivers,
        })
    }

    pub fn update(
        &mut self,
        camera_position: Vec3,
        plant_world: &PlantWorld,
        overlay: EvolutionOverlayMode,
    ) {
        // Newly generated chunks take their plant list from the resident
        // PlantWorld (the whole-world store), reconstructed into this raw
        // chunk's span. Terrain is still generated per chunk for the mesh.
        for mut chunk in self.loader.poll() {
            let plants = plant_world.instances_for(chunk.coord, &chunk.terrain, overlay);
            chunk.content.set_plants(plants);
            self.loaded.insert(chunk.coord, chunk);
        }

        self.center_chunk = world_to_chunk(camera_position);
        let required = required_coords(self.center_chunk, self.load_radius);

        self.loaded.retain(|coord, _| required.contains(coord));
        self.loader.cancel_outside(&required);

        for &coord in &required {
            if !self.loaded.contains_key(&coord) {
                self.loader.dispatch(coord, self.seed);
            }
        }
    }

    pub fn chunks(&self) -> &HashMap<IVec2, ChunkData> {
        &self.loaded
    }

    /// Re-read every loaded chunk's plant list from the resident PlantWorld after
    /// a global growth/spread pass changed it. `set_plants` only bumps a chunk's
    /// revision when its plants actually change, so the renderer re-uploads just
    /// the chunks that moved.
    pub fn refresh_loaded_from_plant_world(
        &mut self,
        plant_world: &PlantWorld,
        overlay: EvolutionOverlayMode,
    ) {
        for (coord, chunk) in self.loaded.iter_mut() {
            let plants = plant_world.instances_for(*coord, &chunk.terrain, overlay);
            chunk.content.set_plants(plants);
        }
    }

    pub fn refresh_changed_from_plant_world(
        &mut self,
        plant_world: &PlantWorld,
        overlay: EvolutionOverlayMode,
        changed_chunks: &[usize],
    ) {
        if changed_chunks.is_empty() {
            return;
        }

        let changed: HashSet<usize> = changed_chunks.iter().copied().collect();
        for (coord, chunk) in self.loaded.iter_mut() {
            let canon = crate::world_core::chunk::canonical_chunk(*coord);
            let idx = (canon.y * crate::world_core::chunk::WORLD_SIZE_CHUNKS + canon.x) as usize;
            if changed.contains(&idx) {
                let plants = plant_world.instances_for(*coord, &chunk.terrain, overlay);
                chunk.content.set_plants(plants);
            }
        }
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    pub fn reload_config(&mut self, config: &GameConfig) {
        let new_config = Arc::new(config.clone());
        // River params live in the config, so re-solve the field before
        // rebuilding the loader that bakes it into new chunks.
        let rivers = Arc::new(RiverField::generate(self.seed, config));
        if let Ok(loader) = PlatformLoader::new_loader(
            self.seed,
            self.thread_count,
            Arc::clone(&new_config),
            Arc::clone(&self.registry),
            Arc::clone(&rivers),
        ) {
            self.loaded.clear();
            self.loader = loader;
            self.load_radius = config.world.load_radius;
            self.config = new_config;
            self.rivers = rivers;
        }
    }

    pub fn stats(&self) -> StreamingStats {
        StreamingStats {
            loaded_chunks: self.loaded.len(),
            pending_chunks: self.loader.pending_count(),
            center_chunk: self.center_chunk,
        }
    }
}

pub(super) fn world_to_chunk(position: Vec3) -> IVec2 {
    IVec2::new(
        (position.x / CHUNK_SIZE_METERS).floor() as i32,
        (position.z / CHUNK_SIZE_METERS).floor() as i32,
    )
}

fn required_coords(center: IVec2, radius: i32) -> HashSet<IVec2> {
    let width = (radius * 2 + 1).max(1);
    let mut required = HashSet::with_capacity((width * width) as usize);

    for z in -radius..=radius {
        for x in -radius..=radius {
            required.insert(IVec2::new(center.x + x, center.y + z));
        }
    }

    required
}
