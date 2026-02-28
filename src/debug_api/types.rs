use serde::{Deserialize, Serialize};

pub const API_VERSION: &str = "v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub api_version: String,
    pub debug_api_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiStateResponse {
    pub api_version: String,
    pub telemetry: Option<TelemetrySnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraSnapshot {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkSnapshot {
    pub loaded: usize,
    pub pending: usize,
    pub center: [i32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub frame: u64,
    pub frame_time_ms: f32,
    pub fps: f32,
    pub hour: f32,
    pub day_speed: f32,
    pub camera: CameraSnapshot,
    pub chunks: ChunkSnapshot,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandRequest {
    pub id: String,
    #[serde(flatten)]
    pub command: CommandKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MoveKey {
    W,
    A,
    S,
    D,
    Up,
    Down,
}

impl MoveKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::W => "w",
            Self::A => "a",
            Self::S => "s",
            Self::D => "d",
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandKind {
    SetDaySpeed { value: f32 },
    SetMoveKey { key: MoveKey, pressed: bool },
    SetCameraPosition { x: f32, y: f32, z: f32 },
    SetCameraLook { yaw: f32, pitch: f32 },
    TakeScreenshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandAcceptedResponse {
    pub api_version: String,
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorResponse {
    pub api_version: String,
    pub error: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandAppliedEvent {
    pub id: String,
    pub frame: u64,
    pub ok: bool,
    pub message: String,
    pub day_speed: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ServerEvent {
    Telemetry(TelemetrySnapshot),
    CommandApplied(CommandAppliedEvent),
}

#[cfg(test)]
mod tests {
    use super::CommandRequest;

    #[test]
    fn deserializes_set_move_key_command() {
        let raw = r#"{"id":"cmd-1","type":"set_move_key","key":"w","pressed":true}"#;
        let command: CommandRequest =
            serde_json::from_str(raw).expect("valid set_move_key payload");
        assert_eq!(command.id, "cmd-1");
    }

    #[test]
    fn deserializes_take_screenshot_command() {
        let raw = r#"{"id":"ss-1","type":"take_screenshot"}"#;
        let command: CommandRequest =
            serde_json::from_str(raw).expect("valid take_screenshot payload");
        assert_eq!(command.id, "ss-1");
        assert!(matches!(
            command.command,
            super::CommandKind::TakeScreenshot
        ));
    }

    #[test]
    fn deserializes_set_move_key_up_down() {
        let raw = r#"{"id":"cmd-2","type":"set_move_key","key":"up","pressed":true}"#;
        let command: CommandRequest =
            serde_json::from_str(raw).expect("valid set_move_key up payload");
        assert_eq!(command.id, "cmd-2");
        assert!(matches!(
            command.command,
            super::CommandKind::SetMoveKey {
                key: super::MoveKey::Up,
                pressed: true
            }
        ));

        let raw = r#"{"id":"cmd-3","type":"set_move_key","key":"down","pressed":false}"#;
        let command: CommandRequest =
            serde_json::from_str(raw).expect("valid set_move_key down payload");
        assert!(matches!(
            command.command,
            super::CommandKind::SetMoveKey {
                key: super::MoveKey::Down,
                pressed: false
            }
        ));
    }

    #[test]
    fn deserializes_set_camera_position() {
        let raw = r#"{"id":"tp-1","type":"set_camera_position","x":100.0,"y":200.0,"z":50.0}"#;
        let command: CommandRequest =
            serde_json::from_str(raw).expect("valid set_camera_position payload");
        assert_eq!(command.id, "tp-1");
        assert!(matches!(
            command.command,
            super::CommandKind::SetCameraPosition { x, y, z }
            if (x - 100.0).abs() < f32::EPSILON
                && (y - 200.0).abs() < f32::EPSILON
                && (z - 50.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn deserializes_set_camera_look() {
        let raw = r#"{"id":"lk-1","type":"set_camera_look","yaw":1.5,"pitch":-0.3}"#;
        let command: CommandRequest =
            serde_json::from_str(raw).expect("valid set_camera_look payload");
        assert_eq!(command.id, "lk-1");
        assert!(matches!(
            command.command,
            super::CommandKind::SetCameraLook { yaw, pitch }
            if (yaw - 1.5).abs() < f32::EPSILON
                && (pitch - (-0.3)).abs() < f32::EPSILON
        ));
    }
}
