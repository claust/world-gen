use serde::{Deserialize, Serialize};

use super::storage::Storage;

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

impl SaveData {
    pub fn load(storage: &dyn Storage) -> Option<Self> {
        let contents = storage.load("save")?;
        match serde_json::from_str(&contents) {
            Ok(save) => {
                log::info!("loaded save");
                Some(save)
            }
            Err(e) => {
                log::warn!("failed to parse save: {e}");
                None
            }
        }
    }

    pub fn save(&self, storage: &dyn Storage) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        storage.save("save", &json)?;
        log::info!("saved game state");
        Ok(())
    }
}
