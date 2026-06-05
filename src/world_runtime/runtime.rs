use std::collections::HashMap;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;

#[cfg(target_arch = "wasm32")]
use web_time::Instant;

use glam::{IVec2, Vec3};

use super::plant_world::PlantWorld;
use crate::world_core::chunk::ChunkData;
use crate::world_core::config::GameConfig;
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::lifecycle::GrowthStage;
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
    pub loaded_base_plants: usize,
    pub loaded_visible_plants: usize,
    pub loaded_visible_seedlings: usize,
    pub loaded_visible_young: usize,
    pub loaded_visible_mature: usize,
    /// Total plants across the whole world (every canonical chunk, loaded or not).
    pub world_population: usize,
    /// Canonical chunks holding at least one plant.
    pub world_populated_chunks: usize,
    /// Seedlings added by the most recent spread pass.
    pub spread_last_added: usize,
    /// Wall-clock duration of the most recent growth/spread tick, in ms.
    pub tick_ms: f32,
    /// Approximate resident bytes of the whole-world plant store.
    pub resident_bytes: usize,
    /// Per-biome fill: `(name, percent_saturated, chunk_count)`.
    pub biome_fill: Vec<(&'static str, f32, usize)>,
}

pub struct WorldRuntime {
    streaming: StreamingWorld,
    clock: WorldClock,
    /// Resident plant store for the whole finite world. Render reads loaded
    /// chunks from here; the global growth + spread ticks advance it.
    plant_world: PlantWorld,
    /// Global-clock hour of the last growth tick, so growth is rate-limited to
    /// the sim cadence rather than running every frame.
    last_growth_hour: f64,
    /// Global-clock hour of the last spread pass — the expensive, lower-cadence
    /// half of the tick.
    last_spread_hour: f64,
    /// Wall-clock ms of the most recent growth/spread tick (telemetry).
    last_tick_ms: f32,
    /// Serialized base-world snapshot awaiting persistence, set when a New Game
    /// generated the base fresh (cache miss). The caller drains it via
    /// [`take_pending_base_snapshot`] and writes it to storage on the main
    /// thread; `None` when the base was loaded from an existing cache or this is
    /// a resume (the cache is only authored from a New Game).
    pending_base_snapshot: Option<Vec<u8>>,
}

/// Sim-hours between global growth ticks. Growth is analytic, so a coarse
/// cadence is plenty and keeps the per-frame cost off the hot path.
const GROWTH_TICK_HOURS: f64 = 1.0;

/// Sim-hours between global spread passes (one reproduction round per sim-day).
const SPREAD_TICK_HOURS: f64 = 24.0;

impl WorldRuntime {
    pub fn new(
        config: &GameConfig,
        save: Option<&SaveData>,
        threads: usize,
        registry: Arc<PlantRegistry>,
        storage: &dyn Storage,
        gen_key: u64,
    ) -> anyhow::Result<Self> {
        let spread_bytes = storage.load_bytes("plants");
        let base_bytes = storage.load_bytes("world_base");
        let mut world = Self::build(
            config,
            save,
            threads,
            registry,
            spread_bytes,
            base_bytes,
            gen_key,
            None,
        )?;
        // Persist a freshly generated base world so the next New Game loads it
        // instead of regenerating. Skipped on backends without binary storage
        // (web localStorage), where a write could never succeed — taking the
        // pending bytes still clears them.
        if let Some(bytes) = world.take_pending_base_snapshot() {
            if storage.supports_bytes() {
                if let Err(err) = storage.save_bytes("world_base", &bytes) {
                    log::warn!("failed to cache base world: {err}");
                }
            }
        }
        Ok(world)
    }

    /// Build a world from fully-owned, `Send` inputs so the whole generation can
    /// run on a worker thread off the event loop. `spread_bytes` is the persisted
    /// spread blob, read from storage on the main thread (storage handles are not
    /// `Send`); `progress` receives live per-chunk progress for the loading UI.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    pub fn generate(
        config: GameConfig,
        save: Option<SaveData>,
        threads: usize,
        registry: Arc<PlantRegistry>,
        spread_bytes: Option<Vec<u8>>,
        base_bytes: Option<Vec<u8>>,
        gen_key: u64,
        progress: &crate::world_runtime::GenerationProgress,
    ) -> anyhow::Result<Self> {
        Self::build(
            &config,
            save.as_ref(),
            threads,
            registry,
            spread_bytes,
            base_bytes,
            gen_key,
            Some(progress),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        config: &GameConfig,
        save: Option<&SaveData>,
        threads: usize,
        registry: Arc<PlantRegistry>,
        spread_bytes: Option<Vec<u8>>,
        base_bytes: Option<Vec<u8>>,
        gen_key: u64,
        progress: Option<&crate::world_runtime::GenerationProgress>,
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

        // Base flora for every canonical chunk. The one-time world-creation cost
        // is generating it from the seed (parallel; terrain discarded per chunk),
        // so a New Game caches the result: try that cache first, keyed by the
        // generation inputs and seed, and only regenerate on a miss. The cache is
        // authored from a New Game (`save.is_none()`) — a resume reuses it when
        // its seed matches but never overwrites it.
        let (mut plant_world, pending_base_snapshot) = match base_bytes.as_deref().and_then(|b| {
            PlantWorld::from_base_snapshot(b, gen_key, seed, config, Arc::clone(&registry))
        }) {
            Some(world) => {
                // Repaint the loading map the generation pass would have filled.
                if let Some(p) = progress {
                    world.paint_progress(p);
                }
                log::info!(
                    "base world: loaded {} plants across {} populated chunks from cache",
                    world.population(),
                    world.populated_chunks(),
                );
                (world, None)
            }
            None => {
                // Cache miss. On a New Game, try downloading a prebuilt base
                // before generating from scratch. The download is validated by
                // the same `from_base_snapshot` path as a local cache hit, so a
                // mismatched (wrong seed/gen_key) or corrupt download is rejected
                // and we fall through to local generation.
                let downloaded = if save.is_none() {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        crate::world_core::storage::fetch_base_world()
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        None::<Vec<u8>>
                    }
                } else {
                    None
                };

                match downloaded.and_then(|bytes| {
                    PlantWorld::from_base_snapshot(
                        &bytes,
                        gen_key,
                        seed,
                        config,
                        Arc::clone(&registry),
                    )
                    .map(|world| (world, bytes))
                }) {
                    Some((world, bytes)) => {
                        // Repaint the loading map the generation pass would have filled.
                        if let Some(p) = progress {
                            world.paint_progress(p);
                        }
                        log::info!(
                            "base world: downloaded {} plants across {} populated chunks",
                            world.population(),
                            world.populated_chunks(),
                        );
                        // Persist the downloaded bytes locally so the next New
                        // Game loads them offline instead of re-downloading.
                        (world, Some(bytes))
                    }
                    None => {
                        let world = PlantWorld::generate_base(
                            seed,
                            config,
                            Arc::clone(&registry),
                            threads,
                            progress,
                        );
                        let pending = if save.is_none() {
                            Some(world.serialize_base(gen_key))
                        } else {
                            None
                        };
                        (world, pending)
                    }
                }
            }
        };
        let restored = plant_world.apply_saved_spread_bytes(spread_bytes.as_deref());
        log::info!(
            "PlantWorld: {} plants across {} populated chunks ({restored} restored from save)",
            plant_world.population(),
            plant_world.populated_chunks(),
        );

        Ok(Self {
            streaming: StreamingWorld::new(seed, load_radius, threads, arc_config, registry)?,
            clock: WorldClock::new(start_hour, total_hours, day_speed),
            plant_world,
            last_growth_hour: total_hours,
            last_spread_hour: total_hours,
            last_tick_ms: 0.0,
            pending_base_snapshot,
        })
    }

    /// Take the serialized base-world snapshot staged by a New Game cache miss,
    /// for the caller to persist on the main thread (storage handles are not
    /// `Send`, so generation can't write it directly). `None` after a cache hit,
    /// a resume, or once already taken.
    pub fn take_pending_base_snapshot(&mut self) -> Option<Vec<u8>> {
        self.pending_base_snapshot.take()
    }

    pub fn reload_config(&mut self, config: &GameConfig) {
        self.streaming.reload_config(config);
    }

    pub fn update(&mut self, dt_seconds: f32, camera_position: Vec3) {
        self.clock.update(dt_seconds);

        // `Instant` resolves to `web_time::Instant` on wasm, so the tick is timed
        // on every platform.
        let tick_start = Instant::now();
        let growth = self.tick_plant_world_growth();
        let spread = self.tick_plant_world_spread();
        // A pass *ran* if it was due; it *changed* the world if it returned `true`.
        // Time every actual pass so `tick_ms` reflects the latest tick, not the
        // latest visible change.
        if growth.is_some() || spread.is_some() {
            self.last_tick_ms = tick_start.elapsed().as_secs_f32() * 1000.0;
        }
        let changed = growth.unwrap_or(false) || spread.unwrap_or(false);

        // Stream chunks around the camera; newly loaded chunks read their plants
        // from the resident PlantWorld.
        self.streaming.update(camera_position, &self.plant_world);
        // If the global sim changed the world, refresh the already-loaded chunks
        // so growth stage changes and new seedlings show up.
        if changed {
            self.streaming
                .refresh_loaded_from_plant_world(&self.plant_world);
        }
    }

    /// Advance global growth, rate-limited to [`GROWTH_TICK_HOURS`] of sim time
    /// and capped to one pass per call so a high `day_speed` can't spin it every
    /// frame. Growth is analytic, so each pass is cheap. Returns `None` if no pass
    /// was due, else `Some(changed)`.
    fn tick_plant_world_growth(&mut self) -> Option<bool> {
        let now = self.clock.total_hours();
        if now - self.last_growth_hour < GROWTH_TICK_HOURS {
            return None;
        }
        self.last_growth_hour = now;
        Some(self.plant_world.tick_growth(now))
    }

    /// Run a global spread pass, rate-limited to [`SPREAD_TICK_HOURS`] of sim
    /// time and capped to one pass per call (the pass is the expensive part, so a
    /// high `day_speed` lets sim-time lag rather than running it many times per
    /// frame). Returns `None` if no pass was due, else `Some(added)` where `added`
    /// is whether any seedling was placed.
    fn tick_plant_world_spread(&mut self) -> Option<bool> {
        let now = self.clock.total_hours();
        if now - self.last_spread_hour < SPREAD_TICK_HOURS {
            return None;
        }
        self.last_spread_hour = now;
        Some(self.plant_world.tick_spread(now))
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
                    GrowthStage::Seedling => loaded_visible_seedlings += 1,
                    GrowthStage::Young => loaded_visible_young += 1,
                    GrowthStage::Mature => loaded_visible_mature += 1,
                }
            }
        }

        RuntimeStats {
            loaded_chunks: streaming.loaded_chunks,
            pending_chunks: streaming.pending_chunks,
            center_chunk: streaming.center_chunk,
            hour: self.clock.hour(),
            loaded_base_plants,
            loaded_visible_plants,
            loaded_visible_seedlings,
            loaded_visible_young,
            loaded_visible_mature,
            world_population: self.plant_world.population(),
            world_populated_chunks: self.plant_world.populated_chunks(),
            spread_last_added: self.plant_world.last_spread_added(),
            tick_ms: self.last_tick_ms,
            resident_bytes: self.plant_world.resident_bytes(),
            biome_fill: self.plant_world.biome_fill_percents(),
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

    pub fn save_plants(&self, storage: &dyn Storage) -> anyhow::Result<()> {
        self.plant_world.save_spread(storage)
    }
}
