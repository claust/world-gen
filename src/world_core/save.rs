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
    pub total_hours: f64,
}

impl SaveData {
    pub fn load(storage: &dyn Storage) -> Option<Self> {
        let contents = storage.load("save")?;
        match serde_json::from_str::<SaveDataCompat>(&contents) {
            Ok(save) => {
                log::info!("loaded save");
                Some(save.into_current())
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

#[derive(Debug, Clone, Deserialize)]
struct SaveDataCompat {
    camera: CameraSave,
    world: WorldSaveCompat,
}

#[derive(Debug, Clone, Deserialize)]
struct WorldSaveCompat {
    seed: u32,
    hour: f32,
    day_speed: f32,
    total_hours: Option<f64>,
}

impl SaveDataCompat {
    fn into_current(self) -> SaveData {
        SaveData {
            camera: self.camera,
            world: WorldSave {
                seed: self.world.seed,
                hour: self.world.hour,
                day_speed: self.world.day_speed,
                total_hours: self.world.total_hours.unwrap_or(self.world.hour as f64),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SaveData, WorldSave};
    use crate::world_core::storage::Storage;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemoryStorage {
        data: RefCell<HashMap<String, String>>,
    }

    impl Storage for MemoryStorage {
        fn load(&self, key: &str) -> Option<String> {
            self.data.borrow().get(key).cloned()
        }

        fn save(&self, key: &str, data: &str) -> anyhow::Result<()> {
            self.data
                .borrow_mut()
                .insert(key.to_string(), data.to_string());
            Ok(())
        }
    }

    #[test]
    fn old_save_without_total_hours_migrates_from_hour() {
        let storage = MemoryStorage::default();
        storage
            .save(
                "save",
                r#"{
  "camera": { "position": [1.0, 2.0, 3.0], "yaw": 4.0, "pitch": 5.0 },
  "world": { "seed": 42, "hour": 7.5, "day_speed": 3.0 }
}"#,
            )
            .unwrap();

        let save = SaveData::load(&storage).expect("save should load");
        assert!((save.world.total_hours - 7.5).abs() < 1e-9);
    }

    #[test]
    fn new_save_round_trips_total_hours() {
        let storage = MemoryStorage::default();
        let save = SaveData {
            camera: super::CameraSave {
                position: [1.0, 2.0, 3.0],
                yaw: 4.0,
                pitch: 5.0,
            },
            world: WorldSave {
                seed: 42,
                hour: 7.5,
                day_speed: 3.0,
                total_hours: 123.25,
            },
        };

        save.save(&storage).unwrap();
        let loaded = SaveData::load(&storage).expect("save should reload");
        assert!((loaded.world.total_hours - 123.25).abs() < 1e-9);
    }

    #[test]
    fn saved_json_includes_total_hours() {
        let storage = MemoryStorage::default();
        let save = SaveData {
            camera: super::CameraSave {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            },
            world: WorldSave {
                seed: 1,
                hour: 2.0,
                day_speed: 3.0,
                total_hours: 4.0,
            },
        };

        save.save(&storage).unwrap();
        let json = storage.load("save").expect("saved json should exist");
        assert!(json.contains("\"total_hours\": 4.0"));
    }
}
