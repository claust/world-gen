use glam::Vec3;
use std::time::{Duration, Instant};

use crate::debug_api::{
    CameraSnapshot, ChunkSnapshot, CommandAppliedEvent, CommandKind, MoveKey, ObjectKind,
    PressableKey, TelemetrySnapshot,
};
use crate::renderer_wgpu::camera::MoveDirection;
use crate::world_runtime::RuntimeStats;

use super::AppState;

impl AppState {
    pub(super) fn apply_debug_commands(&mut self) {
        let commands: Vec<_> = self
            .debug_api
            .as_mut()
            .map(|api| api.drain_commands())
            .unwrap_or_default();

        for command in commands {
            let applied = match command.command {
                CommandKind::SetDaySpeed { value } => {
                    let world = self.world.as_mut().unwrap();
                    match world.set_day_speed(value) {
                        Ok(day_speed) => CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: true,
                            message: "day speed set".to_string(),
                            day_speed: Some(day_speed),
                            object_id: None,
                            object_position: None,
                        },
                        Err(message) => CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: false,
                            message,
                            day_speed: Some(world.day_speed()),
                            object_id: None,
                            object_position: None,
                        },
                    }
                }
                CommandKind::SetMoveKey { key, pressed } => {
                    let direction = match key {
                        MoveKey::W => MoveDirection::Forward,
                        MoveKey::A => MoveDirection::Left,
                        MoveKey::S => MoveDirection::Backward,
                        MoveKey::D => MoveDirection::Right,
                        MoveKey::Up => MoveDirection::Up,
                        MoveKey::Down => MoveDirection::Down,
                    };
                    self.camera_controller.set_remote_move(direction, pressed);

                    CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: true,
                        message: format!(
                            "move key {} {}",
                            key.as_str(),
                            if pressed { "pressed" } else { "released" }
                        ),
                        day_speed: None,
                        object_id: None,
                        object_position: None,
                    }
                }
                CommandKind::SetCameraPosition { x, y, z } => {
                    self.camera.position = glam::Vec3::new(x, y, z);
                    CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: true,
                        message: format!("camera position set to ({:.1}, {:.1}, {:.1})", x, y, z),
                        day_speed: None,
                        object_id: None,
                        object_position: None,
                    }
                }
                CommandKind::SetCameraLook { yaw, pitch } => {
                    self.camera.yaw = yaw;
                    self.camera.pitch = pitch.clamp(-1.54, 1.54);
                    CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: true,
                        message: format!("camera look set to yaw={:.2}, pitch={:.2}", yaw, pitch),
                        day_speed: None,
                        object_id: None,
                        object_position: None,
                    }
                }
                CommandKind::FindNearest { kind } => {
                    let cam_pos = self.camera.position;
                    let mut best: Option<(String, [f32; 3], f32)> = None;

                    for (coord, chunk) in self.world.as_ref().unwrap().chunks() {
                        let items: Box<dyn Iterator<Item = (usize, glam::Vec3)>> = match kind {
                            ObjectKind::House => Box::new(
                                chunk
                                    .content
                                    .houses
                                    .iter()
                                    .enumerate()
                                    .map(|(i, h)| (i, h.position)),
                            ),
                            ObjectKind::Tree => Box::new(
                                chunk
                                    .content
                                    .trees
                                    .iter()
                                    .enumerate()
                                    .map(|(i, t)| (i, t.position)),
                            ),
                            ObjectKind::Fern => Box::new(
                                chunk
                                    .content
                                    .ferns
                                    .iter()
                                    .enumerate()
                                    .map(|(i, f)| (i, f.position)),
                            ),
                        };

                        let prefix = match kind {
                            ObjectKind::House => "house",
                            ObjectKind::Tree => "tree",
                            ObjectKind::Fern => "fern",
                        };

                        for (idx, pos) in items {
                            let dist = cam_pos.distance_squared(pos);
                            let is_closer = best.as_ref().is_none_or(|(_, _, d)| dist < *d);
                            if is_closer {
                                let id = format!("{}-{}_{}-{}", prefix, coord.x, coord.y, idx);
                                best = Some((id, [pos.x, pos.y, pos.z], dist));
                            }
                        }
                    }

                    match best {
                        Some((id, pos, _)) => CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: true,
                            message: format!(
                                "nearest {} at ({:.1}, {:.1}, {:.1})",
                                match kind {
                                    ObjectKind::House => "house",
                                    ObjectKind::Tree => "tree",
                                    ObjectKind::Fern => "fern",
                                },
                                pos[0],
                                pos[1],
                                pos[2]
                            ),
                            day_speed: None,
                            object_id: Some(id),
                            object_position: Some(pos),
                        },
                        None => CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: false,
                            message: format!(
                                "no {} found in loaded chunks",
                                match kind {
                                    ObjectKind::House => "houses",
                                    ObjectKind::Tree => "trees",
                                    ObjectKind::Fern => "ferns",
                                }
                            ),
                            day_speed: None,
                            object_id: None,
                            object_position: None,
                        },
                    }
                }
                CommandKind::LookAtObject {
                    ref object_id,
                    distance,
                } => {
                    let dist = distance.unwrap_or(15.0);
                    let result =
                        parse_and_find_object(object_id, self.world.as_ref().unwrap().chunks());

                    match result {
                        Some(target) => {
                            let offset = glam::Vec3::new(1.0, 0.5, 1.0).normalize() * dist;
                            let cam_pos = target + offset;
                            self.camera.position = cam_pos;

                            let to_target = target - cam_pos;
                            self.camera.yaw = to_target.z.atan2(to_target.x);
                            self.camera.pitch = (to_target.y / to_target.length().max(0.001))
                                .asin()
                                .clamp(-1.54, 1.54);

                            CommandAppliedEvent {
                                id: command.id,
                                frame: self.frame_index,
                                ok: true,
                                message: format!(
                                    "looking at ({:.1}, {:.1}, {:.1}) from {:.1}m",
                                    target.x, target.y, target.z, dist
                                ),
                                day_speed: None,
                                object_id: Some(object_id.clone()),
                                object_position: Some([target.x, target.y, target.z]),
                            }
                        }
                        None => CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: false,
                            message: format!("object '{}' not found", object_id),
                            day_speed: None,
                            object_id: None,
                            object_position: None,
                        },
                    }
                }
                CommandKind::TakeScreenshot => {
                    if self.screenshot_pending.is_some() {
                        CommandAppliedEvent {
                            id: command.id,
                            frame: self.frame_index,
                            ok: false,
                            message: "screenshot already pending".to_string(),
                            day_speed: None,
                            object_id: None,
                            object_position: None,
                        }
                    } else {
                        self.screenshot_pending = Some(command.id);
                        continue;
                    }
                }
                CommandKind::PressKey { key } => {
                    let message = match key {
                        PressableKey::F1 => {
                            self.config_panel.toggle();
                            if self.config_panel.is_visible() {
                                self.release_cursor();
                                "config panel opened".to_string()
                            } else {
                                self.capture_cursor();
                                "config panel closed".to_string()
                            }
                        }
                        PressableKey::Escape => {
                            if self.config_panel.is_visible() {
                                self.config_panel.toggle();
                                self.capture_cursor();
                                "config panel closed".to_string()
                            } else {
                                self.release_cursor();
                                "cursor released".to_string()
                            }
                        }
                    };
                    CommandAppliedEvent {
                        id: command.id,
                        frame: self.frame_index,
                        ok: true,
                        message,
                        day_speed: None,
                        object_id: None,
                        object_position: None,
                    }
                }
            };

            if let Some(api) = &self.debug_api {
                api.publish_command_applied(applied);
            }
        }
    }

    pub(super) fn publish_telemetry_if_due(&mut self, stats: &RuntimeStats) {
        let Some(api) = &self.debug_api else {
            return;
        };

        if self.last_telemetry_emit.elapsed() < Duration::from_millis(100) {
            return;
        }

        let telemetry = TelemetrySnapshot {
            frame: self.frame_index,
            frame_time_ms: self.frame_time_ms,
            fps: 1000.0 / self.frame_time_ms.max(0.01),
            hour: stats.hour,
            day_speed: self.world.as_ref().unwrap().day_speed(),
            camera: CameraSnapshot {
                x: self.camera.position.x,
                y: self.camera.position.y,
                z: self.camera.position.z,
                yaw: self.camera.yaw,
                pitch: self.camera.pitch,
            },
            chunks: ChunkSnapshot {
                loaded: stats.loaded_chunks,
                pending: stats.pending_chunks,
                center: [stats.center_chunk.x, stats.center_chunk.y],
            },
            timestamp_ms: now_timestamp_ms(),
        };

        api.publish_telemetry(telemetry);
        self.last_telemetry_emit = Instant::now();
    }
}

fn parse_and_find_object(
    object_id: &str,
    chunks: &std::collections::HashMap<glam::IVec2, crate::world_core::chunk::ChunkData>,
) -> Option<Vec3> {
    // Format: "{type}-{chunk_x}_{chunk_z}-{index}"
    // Use first '-' and last '-' to handle negative chunk coordinates (e.g. "fern--2_3-0")
    let first_dash = object_id.find('-')?;
    let kind = &object_id[..first_dash];
    let rest = &object_id[first_dash + 1..];
    let last_dash = rest.rfind('-')?;
    let coord_str = &rest[..last_dash];
    let index_str = &rest[last_dash + 1..];

    let underscore = coord_str.find('_')?;
    let cx: i32 = coord_str[..underscore].parse().ok()?;
    let cz: i32 = coord_str[underscore + 1..].parse().ok()?;

    let index: usize = index_str.parse().ok()?;
    let chunk = chunks.get(&glam::IVec2::new(cx, cz))?;

    match kind {
        "house" => chunk.content.houses.get(index).map(|h| h.position),
        "tree" => chunk.content.trees.get(index).map(|t| t.position),
        "fern" => chunk.content.ferns.get(index).map(|f| f.position),
        _ => None,
    }
}

fn now_timestamp_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
