use std::collections::HashMap;
use std::sync::Arc;

use glam::{IVec2, Vec3};

use super::delta_store::{DeltaStore, DeltaStoreStats};
use crate::world_core::chunk::ChunkData;
use crate::world_core::config::GameConfig;
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::save::SaveData;
use crate::world_core::storage::Storage;
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
    pub lifecycle: DeltaStoreStats,
    pub loaded_base_plants: usize,
    pub loaded_visible_plants: usize,
    pub loaded_visible_seedlings: usize,
    pub loaded_visible_young: usize,
    pub loaded_visible_mature: usize,
}

pub struct WorldRuntime {
    streaming: StreamingWorld,
    clock: WorldClock,
    delta_store: DeltaStore,
}

impl WorldRuntime {
    pub fn new(
        config: &GameConfig,
        save: Option<&SaveData>,
        threads: usize,
        registry: Arc<PlantRegistry>,
        storage: &dyn Storage,
    ) -> anyhow::Result<Self> {
        let seed = save.map(|s| s.world.seed).unwrap_or(config.world.seed);
        let start_hour = save
            .map(|s| s.world.hour)
            .unwrap_or(config.world.start_hour);
        let day_speed = save
            .map(|s| s.world.day_speed)
            .unwrap_or(config.world.day_speed);
        let total_hours = save
            .map(|s| s.world.total_hours)
            .unwrap_or(start_hour as f64);
        let load_radius = config.world.load_radius;

        let arc_config = Arc::new(config.clone());

        Ok(Self {
            streaming: StreamingWorld::new(seed, load_radius, threads, arc_config, registry)?,
            clock: WorldClock::new(start_hour, total_hours, day_speed),
            delta_store: DeltaStore::load(storage),
        })
    }

    pub fn reload_config(&mut self, config: &GameConfig) {
        self.streaming.reload_config(config);
    }

    pub fn update(&mut self, dt_seconds: f32, camera_position: Vec3) {
        self.clock.update(dt_seconds);
        self.streaming
            .update(camera_position, &mut self.delta_store);

        let changed_coords = self
            .streaming
            .tick_loaded_chunk_growth(self.clock.total_hours(), &mut self.delta_store);
        for coord in changed_coords {
            self.streaming
                .reassemble_loaded_chunk(coord, &mut self.delta_store);
        }
    }

    pub fn chunks(&self) -> &HashMap<IVec2, ChunkData> {
        self.streaming.chunks()
    }

    pub fn reassemble_loaded_chunk(&mut self, coord: IVec2) -> bool {
        self.streaming
            .reassemble_loaded_chunk(coord, &mut self.delta_store)
    }

    pub fn lighting(&self) -> LightingState {
        LightingState {
            sun_direction: self.clock.sun_direction(),
            ambient: self.clock.ambient_strength(),
        }
    }

    pub fn stats(&self) -> RuntimeStats {
        let streaming = self.streaming.stats();
        let lifecycle = self
            .delta_store
            .stats(self.streaming.chunks().keys().copied());
        let mut loaded_base_plants = 0;
        let mut loaded_visible_plants = 0;
        let mut loaded_visible_seedlings = 0;
        let mut loaded_visible_young = 0;
        let mut loaded_visible_mature = 0;

        for chunk in self.streaming.chunks().values() {
            loaded_base_plants += chunk.content.base_plants.len();
            loaded_visible_plants += chunk.content.plants.len();

            for plant in &chunk.content.plants {
                match plant.growth_stage {
                    crate::world_core::lifecycle::GrowthStage::Seedling => {
                        loaded_visible_seedlings += 1
                    }
                    crate::world_core::lifecycle::GrowthStage::Young => loaded_visible_young += 1,
                    crate::world_core::lifecycle::GrowthStage::Mature => loaded_visible_mature += 1,
                }
            }
        }

        RuntimeStats {
            loaded_chunks: streaming.loaded_chunks,
            pending_chunks: streaming.pending_chunks,
            center_chunk: streaming.center_chunk,
            hour: self.clock.hour(),
            lifecycle,
            loaded_base_plants,
            loaded_visible_plants,
            loaded_visible_seedlings,
            loaded_visible_young,
            loaded_visible_mature,
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

    pub fn total_hours(&self) -> f64 {
        self.clock.total_hours()
    }

    pub fn save_deltas(&self, storage: &dyn Storage) -> anyhow::Result<()> {
        self.delta_store.save(storage)
    }
}
