use std::collections::HashMap;
use std::sync::Arc;

use glam::{IVec2, Vec3};

use super::delta_store::{DeltaStore, DeltaStoreStats};
use super::plant_world::PlantWorld;
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
    /// Total plants across the whole world (every canonical chunk, loaded or not).
    pub world_population: usize,
    /// Canonical chunks holding at least one plant.
    pub world_populated_chunks: usize,
}

pub struct WorldRuntime {
    streaming: StreamingWorld,
    clock: WorldClock,
    delta_store: DeltaStore,
    /// Resident plant store for the whole finite world. Render reads loaded
    /// chunks from here; the global growth tick advances it.
    plant_world: PlantWorld,
    /// Global-clock hour of the last growth tick, so growth is rate-limited to
    /// the sim cadence rather than running every frame.
    last_growth_hour: f64,
}

/// Sim-hours between global growth ticks. Growth is analytic, so a coarse
/// cadence is plenty and keeps the per-frame cost off the hot path.
const GROWTH_TICK_HOURS: f64 = 1.0;

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

        // One-time world-creation cost: generate base flora for every canonical
        // chunk into the resident store (parallel; terrain discarded per chunk).
        let plant_world = PlantWorld::generate_base(seed, config, Arc::clone(&registry), threads);
        log::info!(
            "PlantWorld: {} plants across {} populated chunks",
            plant_world.population(),
            plant_world.populated_chunks(),
        );

        // The delta store is legacy state from the loaded-only model. The global
        // PlantWorld sim does not apply it to rendering yet, so surface it rather
        // than letting a non-empty save silently stop affecting the world. It is
        // still loaded/saved for telemetry continuity and reconciled when spread
        // persistence lands (M4).
        let delta_store = DeltaStore::load(storage);
        if !delta_store.is_empty() {
            log::warn!(
                "loaded legacy delta state; the global PlantWorld sim does not apply it to \
                 rendering — it will be reconciled when spread persistence lands"
            );
        }

        Ok(Self {
            streaming: StreamingWorld::new(seed, load_radius, threads, arc_config, registry)?,
            clock: WorldClock::new(start_hour, total_hours, day_speed),
            delta_store,
            plant_world,
            last_growth_hour: total_hours,
        })
    }

    pub fn reload_config(&mut self, config: &GameConfig) {
        self.streaming.reload_config(config);
    }

    pub fn update(&mut self, dt_seconds: f32, camera_position: Vec3) {
        self.clock.update(dt_seconds);
        self.tick_plant_world_growth();
        // Stream chunks around the camera; loaded chunks read their plants from
        // the resident PlantWorld.
        self.streaming.update(camera_position, &self.plant_world);
    }

    /// Advance global growth, rate-limited to [`GROWTH_TICK_HOURS`] of sim time
    /// and capped to one pass per call so a high `day_speed` can't spin it every
    /// frame. Growth is analytic, so each pass is cheap.
    fn tick_plant_world_growth(&mut self) {
        let now = self.clock.total_hours();
        if now - self.last_growth_hour < GROWTH_TICK_HOURS {
            return;
        }
        self.last_growth_hour = now;
        self.plant_world.tick_growth(now);
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
            world_population: self.plant_world.population(),
            world_populated_chunks: self.plant_world.populated_chunks(),
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
