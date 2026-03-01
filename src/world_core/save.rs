use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveData {
    pub camera: CameraSave,
    pub world: WorldSave,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraSave {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldSave {
    pub seed: u32,
    pub hour: f32,
    pub day_speed: f32,
}

#[cfg(not(target_arch = "wasm32"))]
impl SaveData {
    pub fn load() -> Option<Self> {
        let path = std::path::Path::new("save.json");
        if !path.exists() {
            return None;
        }
        match std::fs::read_to_string(path) {
            Ok(contents) => match serde_json::from_str(&contents) {
                Ok(save) => {
                    log::info!("loaded save.json");
                    Some(save)
                }
                Err(e) => {
                    log::warn!("failed to parse save.json: {e}");
                    None
                }
            },
            Err(e) => {
                log::warn!("failed to read save.json: {e}");
                None
            }
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write("save.json", json)?;
        log::info!("saved game state to save.json");
        Ok(())
    }
}
