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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CommandKind {
    SetDaySpeed { value: f32 },
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
