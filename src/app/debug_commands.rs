use glam::Vec3;
use std::time::{Duration, Instant};

use crate::debug_api::{
    BiomeFill, CameraSnapshot, ChunkSnapshot, CommandAppliedEvent, CommandKind, LifecycleSnapshot,
    MoveKey, ObjectKind, PressableKey, RendererSnapshot, TelemetrySnapshot,
};
use crate::renderer_wgpu::camera::MoveDirection;
use crate::world_core::save::FAVORITE_SLOTS;
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
                        Ok(day_speed) => {
                            let mut evt = CommandAppliedEvent::ok(
                                command.id,
                                self.frame_index,
                                "day speed set".to_string(),
                            );
                            evt.day_speed = Some(day_speed);
                            evt
                        }
                        Err(message) => {
                            let mut evt =
                                CommandAppliedEvent::err(command.id, self.frame_index, message);
                            evt.day_speed = Some(world.day_speed());
                            evt
                        }
                    }
                }
                CommandKind::SetTime { hour } => {
                    let world = self.world.as_mut().unwrap();
                    match world.set_hour(hour) {
                        Ok(current_hour) => {
                            let mut evt = CommandAppliedEvent::ok(
                                command.id,
                                self.frame_index,
                                format!("time of day set to {:.2}h", current_hour),
                            );
                            evt.data = Some(serde_json::json!({ "hour": current_hour }));
                            evt
                        }
                        Err(message) => {
                            let mut evt =
                                CommandAppliedEvent::err(command.id, self.frame_index, message);
                            evt.data = Some(serde_json::json!({ "hour": world.hour() }));
                            evt
                        }
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
                        data: None,
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
                        data: None,
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
                        data: None,
                    }
                }
                CommandKind::MapRightClick { u, v } => {
                    if !self.map_open {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            "map right-click failed: map overlay is not open".to_string(),
                        )
                    } else if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            "map right-click failed: u and v must be in 0..=1".to_string(),
                        )
                    } else {
                        let (x, z) = super::map_uv_to_world_position(u, v);
                        self.teleport_camera_to_map_position(x, z);
                        let mut evt = CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!(
                                "map right-click teleported camera to ({:.1}, {:.1}, {:.1})",
                                self.camera.position.x,
                                self.camera.position.y,
                                self.camera.position.z
                            ),
                        );
                        evt.object_position = Some([
                            self.camera.position.x,
                            self.camera.position.y,
                            self.camera.position.z,
                        ]);
                        evt
                    }
                }
                CommandKind::SaveFavorite { slot } => {
                    if !(1..=FAVORITE_SLOTS as u8).contains(&slot) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("save favorite failed: slot must be in 1..={FAVORITE_SLOTS}"),
                        )
                    } else {
                        self.save_favorite((slot - 1) as usize);
                        CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!("saved favorite position {slot}"),
                        )
                    }
                }
                CommandKind::RecallFavorite { slot } => {
                    if !(1..=FAVORITE_SLOTS as u8).contains(&slot) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("recall favorite failed: slot must be in 1..={FAVORITE_SLOTS}"),
                        )
                    } else if self.favorites[(slot - 1) as usize].is_none() {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("recall favorite failed: slot {slot} is empty"),
                        )
                    } else {
                        self.recall_favorite((slot - 1) as usize);
                        let mut evt = CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!(
                                "recalled favorite {slot} to ({:.1}, {:.1}, {:.1})",
                                self.camera.position.x,
                                self.camera.position.y,
                                self.camera.position.z
                            ),
                        );
                        evt.object_position = Some([
                            self.camera.position.x,
                            self.camera.position.y,
                            self.camera.position.z,
                        ]);
                        evt
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
                            ObjectKind::Tree | ObjectKind::Fern => Box::new(
                                chunk
                                    .content
                                    .plants
                                    .iter()
                                    .enumerate()
                                    .map(|(i, p)| (i, p.position)),
                            ),
                        };

                        let prefix = match kind {
                            ObjectKind::House => "house",
                            ObjectKind::Tree | ObjectKind::Fern => "plant",
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
                                    ObjectKind::Tree | ObjectKind::Fern => "plant",
                                },
                                pos[0],
                                pos[1],
                                pos[2]
                            ),
                            day_speed: None,
                            object_id: Some(id),
                            object_position: Some(pos),
                            data: None,
                        },
                        None => CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!(
                                "no {} found in loaded chunks",
                                match kind {
                                    ObjectKind::House => "houses",
                                    ObjectKind::Tree | ObjectKind::Fern => "plants",
                                }
                            ),
                        ),
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
                                data: None,
                            }
                        }
                        None => CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("object '{}' not found", object_id),
                        ),
                    }
                }
                CommandKind::TakeScreenshot => {
                    if self.screenshot_pending.is_some() {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            "screenshot already pending".to_string(),
                        )
                    } else {
                        self.screenshot_pending = Some(command.id);
                        continue;
                    }
                }
                CommandKind::SaveWorld => match self.save_and_update() {
                    Ok(()) => CommandAppliedEvent::ok(
                        command.id,
                        self.frame_index,
                        "world saved".to_string(),
                    ),
                    Err(e) => CommandAppliedEvent::err(
                        command.id,
                        self.frame_index,
                        format!("save failed: {e}"),
                    ),
                },
                CommandKind::UiSnapshot => {
                    let snapshot = self.ui_registry.take_snapshot(self.screen_name());
                    let data = serde_json::to_value(&snapshot).unwrap_or(serde_json::Value::Null);
                    let mut evt = CommandAppliedEvent::ok(
                        command.id,
                        self.frame_index,
                        format!(
                            "ui snapshot: {} elements on {}",
                            snapshot.elements.len(),
                            snapshot.screen
                        ),
                    );
                    evt.data = Some(data);
                    evt
                }
                CommandKind::UiClick { ref element_id } => {
                    if !self.ui_registry.has_element(element_id) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("ui click failed: element '{}' not found", element_id),
                        )
                    } else {
                        self.ui_registry.push_action(crate::ui::UiAction::Click {
                            element_id: element_id.clone(),
                        });
                        CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!("ui click queued: {}", element_id),
                        )
                    }
                }
                CommandKind::UiSetValue {
                    ref element_id,
                    ref value,
                } => {
                    if !self.ui_registry.has_element(element_id) {
                        CommandAppliedEvent::err(
                            command.id,
                            self.frame_index,
                            format!("ui set_value failed: element '{}' not found", element_id),
                        )
                    } else {
                        self.ui_registry.push_action(crate::ui::UiAction::SetValue {
                            element_id: element_id.clone(),
                            value: value.clone(),
                        });
                        CommandAppliedEvent::ok(
                            command.id,
                            self.frame_index,
                            format!("ui set_value queued: {} = {}", element_id, value),
                        )
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
                            if self.show_help {
                                self.toggle_help();
                                "help overlay closed".to_string()
                            } else if self.map_open {
                                self.toggle_map_overlay();
                                "map overlay closed".to_string()
                            } else if self.config_panel.is_visible() {
                                self.config_panel.toggle();
                                self.capture_cursor();
                                "config panel closed".to_string()
                            } else {
                                self.release_cursor();
                                "cursor released".to_string()
                            }
                        }
                        PressableKey::M => {
                            self.toggle_map_overlay();
                            if self.map_open {
                                "map overlay opened".to_string()
                            } else {
                                "map overlay closed".to_string()
                            }
                        }
                        // Mirrors the in-app `P` shortcut: capture a screenshot
                        // and copy it to the clipboard. The applied event is
                        // deferred (via `continue`) until the render loop
                        // completes the capture, like `TakeScreenshot`.
                        PressableKey::P => {
                            if self.screenshot_pending.is_some() {
                                "screenshot already pending".to_string()
                            } else {
                                self.screenshot_pending = Some(command.id);
                                self.screenshot_to_clipboard = true;
                                continue;
                            }
                        }
                        PressableKey::H => {
                            self.toggle_help();
                            if self.show_help {
                                "help overlay opened".to_string()
                            } else {
                                "help overlay closed".to_string()
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
                        data: None,
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

        let renderer = self.world_renderer.stats();
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
            lifecycle: LifecycleSnapshot {
                world_population: stats.world_population,
                world_populated_chunks: stats.world_populated_chunks,
                spread_last_added: stats.spread_last_added,
                tick_ms: stats.tick_ms,
                resident_mb: stats.resident_bytes as f32 / (1024.0 * 1024.0),
                biome_fill: stats
                    .biome_fill
                    .iter()
                    .map(|(biome, percent, chunks)| BiomeFill {
                        biome: biome.to_string(),
                        percent: *percent,
                        chunks: *chunks,
                    })
                    .collect(),
                loaded_base_plants: stats.loaded_base_plants,
                loaded_visible_plants: stats.loaded_visible_plants,
                loaded_visible_seedlings: stats.loaded_visible_seedlings,
                loaded_visible_young: stats.loaded_visible_young,
                loaded_visible_mature: stats.loaded_visible_mature,
            },
            renderer: RendererSnapshot {
                buffered_mature_plants: renderer.buffered_mature_plants,
                buffered_lod_plants: renderer.buffered_lod_plants,
                buffered_house_instances: renderer.buffered_house_instances,
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
    // Use first '-' and last '-' to handle negative chunk coordinates (e.g. "plant--2_3-0")
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
        "plant" | "tree" | "fern" => chunk.content.plants.get(index).map(|p| p.position),
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
