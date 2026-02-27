use std::collections::HashMap;

use glam::{IVec2, Vec3};

use crate::world_core::chunk::ChunkTerrain;
use crate::world_core::time::WorldClock;
use crate::world_runtime::streaming::StreamingWorld;

pub struct LightingState {
    pub sun_direction: Vec3,
    pub ambient: f32,
}

pub struct RuntimeStats {
    pub loaded_chunks: usize,
    pub pending_chunks: usize,
    pub center_chunk: IVec2,
    pub hour: f32,
}

pub struct WorldRuntime {
    streaming: StreamingWorld,
    clock: WorldClock,
}

impl WorldRuntime {
    pub fn new(
        seed: u32,
        load_radius: i32,
        threads: usize,
        start_hour: f32,
        day_speed: f32,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            streaming: StreamingWorld::new(seed, load_radius, threads)?,
            clock: WorldClock::new(start_hour, day_speed),
        })
    }

    pub fn update(&mut self, dt_seconds: f32, camera_position: Vec3) {
        self.clock.update(dt_seconds);
        self.streaming.update(camera_position);
    }

    pub fn chunks(&self) -> &HashMap<IVec2, ChunkTerrain> {
        self.streaming.chunks()
    }

    pub fn lighting(&self) -> LightingState {
        LightingState {
            sun_direction: self.clock.sun_direction(),
            ambient: self.clock.ambient_strength(),
        }
    }

    pub fn stats(&self) -> RuntimeStats {
        let streaming = self.streaming.stats();
        RuntimeStats {
            loaded_chunks: streaming.loaded_chunks,
            pending_chunks: streaming.pending_chunks,
            center_chunk: streaming.center_chunk,
            hour: self.clock.hour(),
        }
    }
}
