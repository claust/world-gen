//! Ambient nature sound.
//!
//! Three looping beds — surf, birdsong, and an underwater drone — are
//! synthesized procedurally at startup (see [`synth`]) and played through
//! `rodio`. Rather than positioning individual emitters in 3D, each bed's volume
//! is driven per frame from the camera's surroundings: birds swell when trees
//! are nearby and you're close to the ground, surf swells as you approach water,
//! and the underwater drone takes over (ducking the others) when the camera
//! submerges. Target volumes are smoothed over time so layers fade in and out
//! instead of popping.
//!
//! Native only — `rodio`/`cpal` audio output is not wired up for the wasm build.

mod synth;

use std::collections::HashMap;

use glam::{IVec2, Vec3};
use rodio::{OutputStream, OutputStreamHandle, Sink, Source};

use crate::world_core::chunk::{ChunkData, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS};
use crate::world_core::config::AudioConfig;

/// Horizontal radius within which trees contribute to the bird volume.
const BIRD_RADIUS: f32 = 60.0;
/// Minimum plant height (m) to count as a tree for birdsong. Trees are 8–25 m
/// while shrubs are ~1.5–2.5 m, so this cleanly excludes dense shrub cover from
/// boosting the chorus.
const BIRD_MIN_TREE_HEIGHT: f32 = 4.0;
/// Weighted tree count at which birds reach full volume.
const BIRD_SCORE_FULL: f32 = 6.0;
/// Height above ground (m) below which birds are at full volume.
const BIRD_HEIGHT_NEAR: f32 = 20.0;
/// Height above ground (m) beyond which birds are silent.
const BIRD_HEIGHT_FAR: f32 = 80.0;

/// 3D distance to the nearest water at which surf reaches full volume.
const SEA_NEAR: f32 = 30.0;
/// 3D distance to the nearest water beyond which surf is silent.
const SEA_FAR: f32 = 260.0;
/// Stride (in grid cells) used when scanning a chunk's heightmap for water.
const SEA_SCAN_STRIDE: usize = 8;

/// Depth (m) below the surface over which the underwater bed ramps to full —
/// matches the renderer's submerge factor so sound and fog onset agree.
const SUBMERGE_RAMP: f32 = 1.5;

/// Time constant (seconds) for the exponential volume smoothing.
const SMOOTH_TAU: f32 = 0.6;

/// Fixed birdsong / surf gains for the start-menu ambient bed (`0..1`). The menu
/// has no loaded world to drive the proximity mix, so a gentle constant bed
/// plays instead — enough that master-volume changes in Settings are audible.
const MENU_BIRD_GAIN: f32 = 0.5;
const MENU_SEA_GAIN: f32 = 0.45;

pub struct AudioSystem {
    // The stream and its handle must stay alive for playback to continue.
    _stream: OutputStream,
    _handle: OutputStreamHandle,
    sea: Sink,
    birds: Sink,
    underwater: Sink,
    sea_gain: f32,
    birds_gain: f32,
    underwater_gain: f32,
}

impl AudioSystem {
    /// Open the default output device and start all three beds (silent). Returns
    /// `None` if no audio device is available — the game runs fine without sound.
    pub fn new() -> Option<Self> {
        let (stream, handle) = match OutputStream::try_default() {
            Ok(v) => v,
            Err(e) => {
                log::warn!("audio disabled: could not open output device ({e})");
                return None;
            }
        };

        let sea = make_loop(&handle, synth::sea_bed(12.0))?;
        let birds = make_loop(&handle, synth::bird_bed(40.0))?;
        let underwater = make_loop(&handle, synth::underwater_bed(30.0))?;

        log::info!("audio ready: procedural surf + birdsong + underwater beds");
        Some(Self {
            _stream: stream,
            _handle: handle,
            sea,
            birds,
            underwater,
            sea_gain: 0.0,
            birds_gain: 0.0,
            underwater_gain: 0.0,
        })
    }

    /// Recompute target volumes from the camera's surroundings and ease the live
    /// volumes toward them. Call once per frame while playing.
    pub fn update(
        &mut self,
        dt: f32,
        camera: Vec3,
        chunks: &HashMap<IVec2, ChunkData>,
        sea_level: f32,
        cfg: &AudioConfig,
    ) {
        self.sea.play();
        self.birds.play();
        self.underwater.play();

        let (bird_target, sea_target, submerge_target) = if cfg.enabled {
            let submerge = ((sea_level - camera.y) / SUBMERGE_RAMP).clamp(0.0, 1.0);
            (
                bird_target(camera, chunks),
                sea_target(camera, chunks, sea_level),
                submerge,
            )
        } else {
            (0.0, 0.0, 0.0)
        };

        // Frame-rate independent exponential approach toward the target.
        let k = 1.0 - (-dt / SMOOTH_TAU).exp();
        self.birds_gain += (bird_target - self.birds_gain) * k;
        self.sea_gain += (sea_target - self.sea_gain) * k;
        self.underwater_gain += (submerge_target - self.underwater_gain) * k;

        // Underwater the airborne beds are muffled away — birds entirely, surf
        // down to a faint trace — so the drone dominates once submerged.
        let duck_birds = 1.0 - self.underwater_gain;
        let duck_sea = 1.0 - 0.7 * self.underwater_gain;

        self.birds
            .set_volume(self.birds_gain * cfg.bird_volume * cfg.master_volume * duck_birds);
        self.sea
            .set_volume(self.sea_gain * cfg.sea_volume * cfg.master_volume * duck_sea);
        self.underwater
            .set_volume(self.underwater_gain * cfg.underwater_volume * cfg.master_volume);
    }

    /// Play a gentle, fixed ambient bed for the start menu (surf + birdsong, no
    /// underwater drone) so master-volume changes in Settings are audible before
    /// entering the world. Eases in from silence like [`update`].
    pub fn update_menu(&mut self, dt: f32, cfg: &AudioConfig) {
        self.sea.play();
        self.birds.play();

        let (bird_target, sea_target) = if cfg.enabled {
            (MENU_BIRD_GAIN, MENU_SEA_GAIN)
        } else {
            (0.0, 0.0)
        };

        let k = 1.0 - (-dt / SMOOTH_TAU).exp();
        self.birds_gain += (bird_target - self.birds_gain) * k;
        self.sea_gain += (sea_target - self.sea_gain) * k;
        self.underwater_gain += (0.0 - self.underwater_gain) * k;

        self.birds
            .set_volume(self.birds_gain * cfg.bird_volume * cfg.master_volume);
        self.sea
            .set_volume(self.sea_gain * cfg.sea_volume * cfg.master_volume);
        self.underwater.set_volume(0.0);
    }

    /// Pause all beds (used when not in the playing screen).
    pub fn pause(&self) {
        self.sea.pause();
        self.birds.pause();
        self.underwater.pause();
    }
}

/// Build a sink playing `buf` (mono, [`synth::SAMPLE_RATE`]) on infinite repeat,
/// starting silent.
fn make_loop(handle: &OutputStreamHandle, buf: Vec<f32>) -> Option<Sink> {
    let sink = match Sink::try_new(handle) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("audio disabled: could not create sink ({e})");
            return None;
        }
    };
    let source = rodio::buffer::SamplesBuffer::new(1, synth::SAMPLE_RATE, buf).repeat_infinite();
    sink.append(source);
    sink.set_volume(0.0);
    Some(sink)
}

/// The raw chunk coordinate containing a world position.
fn chunk_of(p: Vec3) -> IVec2 {
    IVec2::new(
        (p.x / CHUNK_SIZE_METERS).floor() as i32,
        (p.z / CHUNK_SIZE_METERS).floor() as i32,
    )
}

/// Bird volume target in `0..1`: a distance-weighted count of nearby trees,
/// attenuated when the camera climbs well above the ground.
fn bird_target(camera: Vec3, chunks: &HashMap<IVec2, ChunkData>) -> f32 {
    let center = chunk_of(camera);
    let mut score = 0.0f32;

    'scan: for dz in -1..=1 {
        for dx in -1..=1 {
            let coord = IVec2::new(center.x + dx, center.y + dz);
            let Some(chunk) = chunks.get(&coord) else {
                continue;
            };
            for plant in chunk
                .content
                .base_plants
                .iter()
                .chain(chunk.content.plants.iter())
            {
                if plant.height < BIRD_MIN_TREE_HEIGHT {
                    continue; // shrubs don't host the dawn chorus
                }
                let dxp = plant.position.x - camera.x;
                let dzp = plant.position.z - camera.z;
                let d2 = dxp * dxp + dzp * dzp;
                if d2 < BIRD_RADIUS * BIRD_RADIUS {
                    score += 1.0 - d2.sqrt() / BIRD_RADIUS;
                    if score >= BIRD_SCORE_FULL {
                        break 'scan;
                    }
                }
            }
        }
    }

    let proximity = (score / BIRD_SCORE_FULL).clamp(0.0, 1.0);

    // Fade out as the camera rises above the trees.
    let altitude = match ground_height(camera, chunks) {
        Some(ground) => {
            let above = camera.y - ground;
            1.0 - ((above - BIRD_HEIGHT_NEAR) / (BIRD_HEIGHT_FAR - BIRD_HEIGHT_NEAR))
                .clamp(0.0, 1.0)
        }
        None => 1.0,
    };

    proximity * altitude
}

/// Terrain height directly under the camera, if its chunk is loaded.
fn ground_height(camera: Vec3, chunks: &HashMap<IVec2, ChunkData>) -> Option<f32> {
    let coord = chunk_of(camera);
    let chunk = chunks.get(&coord)?;
    let local_x = camera.x - coord.x as f32 * CHUNK_SIZE_METERS;
    let local_z = camera.z - coord.y as f32 * CHUNK_SIZE_METERS;
    Some(chunk.terrain.height_at_world(local_x, local_z))
}

/// Surf volume target in `0..1`, from the 3D distance to the nearest submerged
/// terrain sample in the camera's chunk neighbourhood.
fn sea_target(camera: Vec3, chunks: &HashMap<IVec2, ChunkData>, sea_level: f32) -> f32 {
    let center = chunk_of(camera);
    let side = CHUNK_GRID_RESOLUTION;
    let cell = CHUNK_SIZE_METERS / (side - 1) as f32;
    let mut best2 = f32::INFINITY;

    for dz in -1..=1 {
        for dx in -1..=1 {
            let coord = IVec2::new(center.x + dx, center.y + dz);
            let Some(chunk) = chunks.get(&coord) else {
                continue;
            };
            if !chunk.terrain.has_water {
                continue;
            }
            let base_x = coord.x as f32 * CHUNK_SIZE_METERS;
            let base_z = coord.y as f32 * CHUNK_SIZE_METERS;
            let mut iz = 0;
            while iz < side {
                let mut ix = 0;
                while ix < side {
                    if chunk.terrain.heights[iz * side + ix] < sea_level {
                        let dxs = base_x + ix as f32 * cell - camera.x;
                        let dzs = base_z + iz as f32 * cell - camera.z;
                        let dys = sea_level - camera.y;
                        let d2 = dxs * dxs + dys * dys + dzs * dzs;
                        if d2 < best2 {
                            best2 = d2;
                        }
                    }
                    ix += SEA_SCAN_STRIDE;
                }
                iz += SEA_SCAN_STRIDE;
            }
        }
    }

    if !best2.is_finite() {
        return 0.0;
    }
    let d = best2.sqrt();
    let t = ((SEA_FAR - d) / (SEA_FAR - SEA_NEAR)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t) // smoothstep
}
