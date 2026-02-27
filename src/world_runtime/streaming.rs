use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};

use glam::{IVec2, Vec3};
use rayon::{ThreadPool, ThreadPoolBuilder};

use crate::world_core::chunk::{ChunkData, CHUNK_SIZE_METERS};
use crate::world_core::chunk_generator::ChunkGenerator;

pub struct StreamingStats {
    pub loaded_chunks: usize,
    pub pending_chunks: usize,
    pub center_chunk: IVec2,
}

pub struct StreamingWorld {
    seed: u32,
    load_radius: i32,
    pool: ThreadPool,
    sender: Sender<ChunkData>,
    receiver: Receiver<ChunkData>,
    loaded: HashMap<IVec2, ChunkData>,
    pending: HashSet<IVec2>,
    center_chunk: IVec2,
}

impl StreamingWorld {
    pub fn new(seed: u32, load_radius: i32, threads: usize) -> anyhow::Result<Self> {
        let pool = ThreadPoolBuilder::new()
            .num_threads(threads.max(1))
            .thread_name(|i| format!("chunk-gen-{i}"))
            .build()?;

        let (sender, receiver) = mpsc::channel();

        let generator = ChunkGenerator::new(seed);
        let center_chunk = IVec2::ZERO;
        let initial_chunk = generator.generate_chunk(center_chunk);
        let mut loaded = HashMap::with_capacity(1);
        loaded.insert(center_chunk, initial_chunk);

        Ok(Self {
            seed,
            load_radius,
            pool,
            sender,
            receiver,
            loaded,
            pending: HashSet::new(),
            center_chunk,
        })
    }

    pub fn update(&mut self, camera_position: Vec3) {
        self.drain_completed();

        self.center_chunk = world_to_chunk(camera_position);
        let required = required_coords(self.center_chunk, self.load_radius);

        self.loaded.retain(|coord, _| required.contains(coord));
        self.pending.retain(|coord| required.contains(coord));

        for coord in required {
            if self.loaded.contains_key(&coord) || self.pending.contains(&coord) {
                continue;
            }

            let tx = self.sender.clone();
            let seed = self.seed;
            self.pending.insert(coord);

            self.pool.spawn(move || {
                let generator = ChunkGenerator::new(seed);
                let chunk = generator.generate_chunk(coord);
                let _ = tx.send(chunk);
            });
        }
    }

    fn drain_completed(&mut self) {
        while let Ok(chunk) = self.receiver.try_recv() {
            self.pending.remove(&chunk.coord);
            self.loaded.insert(chunk.coord, chunk);
        }
    }

    pub fn chunks(&self) -> &HashMap<IVec2, ChunkData> {
        &self.loaded
    }

    pub fn stats(&self) -> StreamingStats {
        StreamingStats {
            loaded_chunks: self.loaded.len(),
            pending_chunks: self.pending.len(),
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
