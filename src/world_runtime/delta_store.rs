use std::collections::HashMap;

use glam::IVec2;
use serde::{Deserialize, Serialize};

use crate::world_core::lifecycle::ChunkDelta;
use crate::world_core::storage::Storage;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeltaStoreStats {
    pub total_chunks: usize,
    pub loaded_chunks: usize,
    pub total_plants: usize,
    pub loaded_plants: usize,
    pub seedlings: usize,
    pub young: usize,
    pub mature: usize,
}

#[derive(Default)]
pub struct DeltaStore {
    deltas: HashMap<IVec2, ChunkDelta>,
}

impl DeltaStore {
    pub fn get(&self, coord: &IVec2) -> Option<&ChunkDelta> {
        self.deltas.get(coord)
    }

    pub fn get_or_create(&mut self, coord: IVec2) -> &mut ChunkDelta {
        self.deltas.entry(coord).or_default()
    }

    pub fn remove(&mut self, coord: &IVec2) -> Option<ChunkDelta> {
        self.deltas.remove(coord)
    }

    pub fn is_empty(&self) -> bool {
        self.deltas.values().all(ChunkDelta::is_empty)
    }

    pub fn stats<I>(&self, loaded_coords: I) -> DeltaStoreStats
    where
        I: IntoIterator<Item = IVec2>,
    {
        let loaded_coords: std::collections::HashSet<_> = loaded_coords.into_iter().collect();
        let mut stats = DeltaStoreStats::default();

        for (coord, delta) in &self.deltas {
            if delta.is_empty() {
                continue;
            }

            stats.total_chunks += 1;
            let is_loaded = loaded_coords.contains(coord);
            if is_loaded {
                stats.loaded_chunks += 1;
            }

            for plant in &delta.added_plants {
                stats.total_plants += 1;
                if is_loaded {
                    stats.loaded_plants += 1;
                }

                match plant.stage {
                    crate::world_core::lifecycle::GrowthStage::Seedling => stats.seedlings += 1,
                    crate::world_core::lifecycle::GrowthStage::Young => stats.young += 1,
                    crate::world_core::lifecycle::GrowthStage::Mature => stats.mature += 1,
                }
            }
        }

        stats
    }

    pub fn save(&self, storage: &dyn Storage) -> anyhow::Result<()> {
        let serialized = DeltaStoreSerde {
            deltas: self
                .deltas
                .iter()
                .filter(|(_, delta)| !delta.is_empty())
                .map(|(coord, delta)| (coord_key(*coord), delta.clone()))
                .collect(),
        };
        let json = serde_json::to_string_pretty(&serialized)?;
        storage.save("deltas", &json)
    }

    pub fn load(storage: &dyn Storage) -> Self {
        let Some(contents) = storage.load("deltas") else {
            return Self::default();
        };

        match serde_json::from_str::<DeltaStoreSerde>(&contents) {
            Ok(data) => {
                let deltas = data
                    .deltas
                    .into_iter()
                    .filter_map(|(key, delta)| parse_coord_key(&key).map(|coord| (coord, delta)))
                    .collect();
                Self { deltas }
            }
            Err(error) => {
                log::warn!("failed to parse deltas: {error}");
                Self::default()
            }
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct DeltaStoreSerde {
    deltas: HashMap<String, ChunkDelta>,
}

fn coord_key(coord: IVec2) -> String {
    format!("{},{}", coord.x, coord.y)
}

fn parse_coord_key(key: &str) -> Option<IVec2> {
    let (x, y) = key.split_once(',')?;
    let x = x.parse().ok()?;
    let y = y.parse().ok()?;
    Some(IVec2::new(x, y))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use glam::{IVec2, Vec3};

    use super::{DeltaStore, DeltaStoreStats};
    use crate::world_core::lifecycle::{ChunkDelta, DeltaPlant, GrowthStage};
    use crate::world_core::storage::Storage;

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
    fn delta_store_round_trips_non_empty_entries() {
        let storage = MemoryStorage::default();
        let mut store = DeltaStore::default();
        *store.get_or_create(IVec2::new(2, -3)) = ChunkDelta {
            removed_base: vec![1, 4],
            added_plants: vec![DeltaPlant {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: 0.25,
                height: 7.0,
                species_index: 2,
                stage: GrowthStage::Seedling,
                born_hour: 11.0,
            }],
            last_sim_hour: 5.0,
        };
        let _ = store.get_or_create(IVec2::new(0, 0));

        store.save(&storage).unwrap();
        let json = storage.load("deltas").expect("deltas should be saved");
        assert!(json.contains("\"2,-3\""));
        assert!(!json.contains("\"0,0\""));

        let loaded = DeltaStore::load(&storage);
        let delta = loaded.get(&IVec2::new(2, -3)).expect("delta should reload");
        assert_eq!(delta.removed_base, vec![1, 4]);
        assert_eq!(delta.added_plants.len(), 1);
        assert_eq!(delta.added_plants[0].stage, GrowthStage::Seedling);
    }

    #[test]
    fn delta_store_stats_counts_loaded_and_stages() {
        let mut store = DeltaStore::default();
        *store.get_or_create(IVec2::new(2, -3)) = ChunkDelta {
            removed_base: vec![1],
            added_plants: vec![
                DeltaPlant {
                    position: Vec3::new(1.0, 2.0, 3.0),
                    rotation: 0.25,
                    height: 7.0,
                    species_index: 2,
                    stage: GrowthStage::Seedling,
                    born_hour: 11.0,
                },
                DeltaPlant {
                    position: Vec3::new(4.0, 5.0, 6.0),
                    rotation: 0.5,
                    height: 9.0,
                    species_index: 1,
                    stage: GrowthStage::Young,
                    born_hour: 15.0,
                },
            ],
            last_sim_hour: 5.0,
        };
        *store.get_or_create(IVec2::new(8, 9)) = ChunkDelta {
            removed_base: Vec::new(),
            added_plants: vec![DeltaPlant {
                position: Vec3::new(7.0, 8.0, 9.0),
                rotation: 0.75,
                height: 11.0,
                species_index: 0,
                stage: GrowthStage::Mature,
                born_hour: 18.0,
            }],
            last_sim_hour: 6.0,
        };
        let _ = store.get_or_create(IVec2::new(0, 0));

        let stats = store.stats([IVec2::new(2, -3)]);

        assert_eq!(
            stats,
            DeltaStoreStats {
                total_chunks: 2,
                loaded_chunks: 1,
                total_plants: 3,
                loaded_plants: 2,
                seedlings: 1,
                young: 1,
                mature: 1,
            }
        );
    }
}
