use std::collections::HashMap;
use std::sync::Arc;

use glam::{IVec2, Vec3};

use crate::world_core::chunk::ChunkData;
use crate::world_core::config::GameConfig;
use crate::world_core::save::SaveData;
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
        config: &GameConfig,
        save: Option<&SaveData>,
        threads: usize,
    ) -> anyhow::Result<Self> {
        let seed = save.map(|s| s.world.seed).unwrap_or(config.world.seed);
        let start_hour = save
            .map(|s| s.world.hour)
            .unwrap_or(config.world.start_hour);
        let day_speed = save
            .map(|s| s.world.day_speed)
            .unwrap_or(config.world.day_speed);
        let load_radius = config.world.load_radius;

        let arc_config = Arc::new(config.clone());

        Ok(Self {
            streaming: StreamingWorld::new(seed, load_radius, threads, arc_config)?,
            clock: WorldClock::new(start_hour, day_speed),
        })
    }

    pub fn update(&mut self, dt_seconds: f32, camera_position: Vec3) {
        self.clock.update(dt_seconds);
        self.streaming.update(camera_position);
    }

    pub fn chunks(&self) -> &HashMap<IVec2, ChunkData> {
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

    pub fn seed(&self) -> u32 {
        self.streaming.seed()
    }

    pub fn day_speed(&self) -> f32 {
        self.clock.day_speed()
    }

    pub fn set_day_speed(&mut self, value: f32) -> Result<f32, String> {
        if !value.is_finite() {
            return Err("day speed must be a finite number".to_string());
        }
        if !(0.0..=2000.0).contains(&value) {
            return Err("day speed must be between 0.0 and 2000.0".to_string());
        }

        self.clock.set_day_speed(value);
        Ok(self.clock.day_speed())
    }

    pub fn hour(&self) -> f32 {
        self.clock.hour()
    }
}
