use std::collections::HashSet;
use std::sync::Arc;

use glam::IVec2;

use crate::world_core::chunk::ChunkData;
use crate::world_core::chunk_generator::ChunkGenerator;
use crate::world_core::config::GameConfig;
use crate::world_core::herbarium::PlantRegistry;

// ---------------------------------------------------------------------------
// ChunkLoader trait — abstracts platform-specific chunk generation strategy
// ---------------------------------------------------------------------------

pub(super) trait ChunkLoader {
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

#[cfg(not(target_arch = "wasm32"))]
pub(super) type PlatformLoader = threaded::ThreadedLoader;
#[cfg(target_arch = "wasm32")]
pub(super) type PlatformLoader = sync::SyncLoader;
