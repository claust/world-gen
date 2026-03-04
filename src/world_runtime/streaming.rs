use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{IVec2, Vec3};

use crate::world_core::chunk::{ChunkData, CHUNK_SIZE_METERS};
use crate::world_core::chunk_generator::ChunkGenerator;
use crate::world_core::config::GameConfig;
use crate::world_core::herbarium::PlantRegistry;

pub struct StreamingStats {
    pub loaded_chunks: usize,
    pub pending_chunks: usize,
    pub center_chunk: IVec2,
}

// ---------------------------------------------------------------------------
// ChunkLoader trait — abstracts platform-specific chunk generation strategy
// ---------------------------------------------------------------------------

trait ChunkLoader {
    fn new_loader(
        seed: u32,
        threads: usize,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
    ) -> anyhow::Result<Self>
    where
        Self: Sized;
    fn dispatch(&mut self, coord: IVec2, seed: u32);
    fn poll(&mut self) -> Vec<ChunkData>;
    fn pending_count(&self) -> usize;
    fn cancel_outside(&mut self, required: &HashSet<IVec2>);
}

// ---------------------------------------------------------------------------
// Native: threaded chunk generation via rayon
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
mod threaded {
    use super::*;
    use std::sync::mpsc::{self, Receiver, Sender};

    use rayon::{ThreadPool, ThreadPoolBuilder};

    pub struct ThreadedLoader {
        pool: ThreadPool,
        sender: Sender<ChunkData>,
        receiver: Receiver<ChunkData>,
        pending: HashSet<IVec2>,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
    }

    impl ChunkLoader for ThreadedLoader {
        fn new_loader(
            seed: u32,
            threads: usize,
            config: Arc<GameConfig>,
            registry: Arc<PlantRegistry>,
        ) -> anyhow::Result<Self> {
            let _ = seed; // seed is passed per-dispatch, not stored
            let pool = ThreadPoolBuilder::new()
                .num_threads(threads.max(1))
                .thread_name(|i| format!("chunk-gen-{i}"))
                .build()?;
            let (sender, receiver) = mpsc::channel();
            Ok(Self {
                pool,
                sender,
                receiver,
                pending: HashSet::new(),
                config,
                registry,
            })
        }

        fn dispatch(&mut self, coord: IVec2, seed: u32) {
            if self.pending.contains(&coord) {
                return;
            }
            self.pending.insert(coord);
            let tx = self.sender.clone();
            let config = Arc::clone(&self.config);
            let registry = Arc::clone(&self.registry);
            self.pool.spawn(move || {
                let generator = ChunkGenerator::new(seed, &config, registry);
                let chunk = generator.generate_chunk(coord);
                let _ = tx.send(chunk);
            });
        }

        fn poll(&mut self) -> Vec<ChunkData> {
            let mut completed = Vec::new();
            while let Ok(chunk) = self.receiver.try_recv() {
                self.pending.remove(&chunk.coord);
                completed.push(chunk);
            }
            completed
        }

        fn pending_count(&self) -> usize {
            self.pending.len()
        }

        fn cancel_outside(&mut self, required: &HashSet<IVec2>) {
            self.pending.retain(|coord| required.contains(coord));
        }
    }
}

// ---------------------------------------------------------------------------
// Wasm: synchronous chunk generation, throttled per frame
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
mod sync {
    use super::*;

    pub struct SyncLoader {
        seed: u32,
        queue: Vec<IVec2>,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
    }

    impl ChunkLoader for SyncLoader {
        fn new_loader(
            seed: u32,
            _threads: usize,
            config: Arc<GameConfig>,
            registry: Arc<PlantRegistry>,
        ) -> anyhow::Result<Self> {
            Ok(Self {
                seed,
                queue: Vec::new(),
                config,
                registry,
            })
        }

        fn dispatch(&mut self, coord: IVec2, _seed: u32) {
            if !self.queue.contains(&coord) {
                self.queue.push(coord);
            }
        }

        fn poll(&mut self) -> Vec<ChunkData> {
            let generator =
                ChunkGenerator::new(self.seed, &self.config, Arc::clone(&self.registry));
            let count = self.queue.len().min(2);
            let coords: Vec<IVec2> = self.queue.drain(..count).collect();
            coords
                .into_iter()
                .map(|coord| generator.generate_chunk(coord))
                .collect()
        }

        fn pending_count(&self) -> usize {
            self.queue.len()
        }

        fn cancel_outside(&mut self, required: &HashSet<IVec2>) {
            self.queue.retain(|coord| required.contains(coord));
        }
    }
}

// ---------------------------------------------------------------------------
// StreamingWorld — unified orchestration, delegates loading to PlatformLoader
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "wasm32"))]
type PlatformLoader = threaded::ThreadedLoader;
#[cfg(target_arch = "wasm32")]
type PlatformLoader = sync::SyncLoader;

pub struct StreamingWorld {
    seed: u32,
    load_radius: i32,
    loaded: HashMap<IVec2, ChunkData>,
    center_chunk: IVec2,
    loader: PlatformLoader,
    thread_count: usize,
    registry: Arc<PlantRegistry>,
}

impl StreamingWorld {
    pub fn new(
        seed: u32,
        load_radius: i32,
        threads: usize,
        config: Arc<GameConfig>,
        registry: Arc<PlantRegistry>,
    ) -> anyhow::Result<Self> {
        let loader =
            PlatformLoader::new_loader(seed, threads, Arc::clone(&config), Arc::clone(&registry))?;

        // No synchronous chunk generation — all chunks (including the center)
        // are dispatched to background threads via update().
        Ok(Self {
            seed,
            load_radius,
            loaded: HashMap::new(),
            center_chunk: IVec2::ZERO,
            loader,
            thread_count: threads,
            registry,
        })
    }

    pub fn update(&mut self, camera_position: Vec3) {
        for chunk in self.loader.poll() {
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

    pub fn seed(&self) -> u32 {
        self.seed
    }

    pub fn reload_config(&mut self, config: &GameConfig) {
        let new_config = Arc::new(config.clone());
        if let Ok(loader) = PlatformLoader::new_loader(
            self.seed,
            self.thread_count,
            new_config,
            Arc::clone(&self.registry),
        ) {
            self.loaded.clear();
            self.loader = loader;
            self.load_radius = config.world.load_radius;
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

fn world_to_chunk(position: Vec3) -> IVec2 {
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
