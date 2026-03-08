use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::world_core::chunk::PlantInstance;
use crate::world_core::herbarium::PlantRegistry;

pub const MAX_CATCH_UP_HOURS: f64 = 500.0;

/// Growth stage of a delta-tracked plant.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum GrowthStage {
    Seedling,
    Young,
    Mature,
}

impl GrowthStage {
    pub const fn scale_factor(self) -> f32 {
        match self {
            Self::Seedling => 0.15,
            Self::Young => 0.50,
            Self::Mature => 1.0,
        }
    }
}

/// A plant that exists in the delta layer (not in the deterministic base).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeltaPlant {
    #[serde(with = "vec3_serde")]
    pub position: Vec3,
    pub rotation: f32,
    pub height: f32,
    pub species_index: usize,
    pub stage: GrowthStage,
    pub born_hour: f64,
}

/// Overlay of modifications to a chunk's deterministic plant content.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChunkDelta {
    pub removed_base: Vec<usize>,
    pub added_plants: Vec<DeltaPlant>,
    pub last_sim_hour: f64,
}

impl ChunkDelta {
    pub fn is_empty(&self) -> bool {
        self.removed_base.is_empty() && self.added_plants.is_empty() && self.last_sim_hour == 0.0
    }

    pub fn prune_removed_base(&mut self, base_len: usize) -> bool {
        let original = self.removed_base.clone();
        self.removed_base.retain(|&index| index < base_len);
        self.removed_base.sort_unstable();
        self.removed_base.dedup();
        self.removed_base != original
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CatchUpWindow {
    pub target_hour: f64,
    pub missed_boundaries: u64,
    pub clamped: bool,
}

pub fn bounded_catch_up_window(
    last_sim_hour: f64,
    current_hour: f64,
    max_catch_up_hours: f64,
) -> CatchUpWindow {
    let current_hour = current_hour.max(last_sim_hour);
    let max_catch_up_hours = max_catch_up_hours.max(0.0);
    let target_hour = current_hour.min(last_sim_hour + max_catch_up_hours);
    let missed_boundaries =
        (target_hour.floor() as i64 - last_sim_hour.floor() as i64).max(0) as u64;

    CatchUpWindow {
        target_hour,
        missed_boundaries,
        clamped: target_hour < current_hour,
    }
}

pub fn growth_stage_for_plant(
    plant: &DeltaPlant,
    sim_hour: f64,
    registry: &PlantRegistry,
) -> GrowthStage {
    let Some(species) = registry.species.get(plant.species_index) else {
        debug_assert!(
            false,
            "delta plant references invalid species index {}",
            plant.species_index
        );
        return plant.stage;
    };

    let age = (sim_hour - plant.born_hour).max(0.0);
    let young_at = species.placement.seedling_hours.max(0.0) as f64;
    let mature_at = young_at + species.placement.young_hours.max(0.0) as f64;

    if age >= mature_at {
        GrowthStage::Mature
    } else if age >= young_at {
        GrowthStage::Young
    } else {
        GrowthStage::Seedling
    }
}

pub fn advance_delta_plant_growth(
    plant: &mut DeltaPlant,
    sim_hour: f64,
    registry: &PlantRegistry,
) -> bool {
    let next_stage = growth_stage_for_plant(plant, sim_hour, registry);
    if next_stage <= plant.stage {
        return false;
    }

    plant.stage = next_stage;
    true
}

pub fn assemble_plants(base: &[PlantInstance], delta: &ChunkDelta) -> Vec<PlantInstance> {
    let mut plants: Vec<PlantInstance> = base
        .iter()
        .enumerate()
        .filter(|(index, _)| delta.removed_base.binary_search(index).is_err())
        .map(|(_, plant)| plant.clone())
        .collect();

    plants.extend(delta.added_plants.iter().map(|plant| PlantInstance {
        position: plant.position,
        rotation: plant.rotation,
        height: plant.height,
        species_index: plant.species_index,
        growth_stage: plant.stage,
    }));

    plants
}

mod vec3_serde {
    use glam::Vec3;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(value: &Vec3, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        [value.x, value.y, value.z].serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec3, D::Error>
    where
        D: Deserializer<'de>,
    {
        let [x, y, z] = <[f32; 3]>::deserialize(deserializer)?;
        Ok(Vec3::new(x, y, z))
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::{
        advance_delta_plant_growth, assemble_plants, bounded_catch_up_window, ChunkDelta,
        DeltaPlant, GrowthStage, MAX_CATCH_UP_HOURS,
    };
    use crate::world_core::chunk::PlantInstance;
    use crate::world_core::herbarium::{Herbarium, PlantRegistry};

    fn test_registry() -> PlantRegistry {
        PlantRegistry::from_herbarium(&Herbarium::default_seeded())
    }

    #[test]
    fn assemble_plants_applies_removed_base_and_added_plants() {
        let base = vec![
            PlantInstance {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: 0.1,
                height: 10.0,
                species_index: 0,
                growth_stage: GrowthStage::Mature,
            },
            PlantInstance {
                position: Vec3::new(4.0, 5.0, 6.0),
                rotation: 0.2,
                height: 12.0,
                species_index: 1,
                growth_stage: GrowthStage::Mature,
            },
        ];
        let delta = ChunkDelta {
            removed_base: vec![0],
            added_plants: vec![DeltaPlant {
                position: Vec3::new(7.0, 8.0, 9.0),
                rotation: 0.3,
                height: 20.0,
                species_index: 2,
                stage: GrowthStage::Young,
                born_hour: 1.0,
            }],
            last_sim_hour: 0.0,
        };

        let plants = assemble_plants(&base, &delta);

        assert_eq!(plants.len(), 2);
        assert_eq!(plants[0].position, base[1].position);
        assert_eq!(plants[1].growth_stage, GrowthStage::Young);
        assert!((plants[1].height - 20.0).abs() < 1e-5);
    }

    #[test]
    fn prune_removed_base_discards_stale_indices() {
        let mut delta = ChunkDelta {
            removed_base: vec![0, 2, 2, 9],
            added_plants: Vec::new(),
            last_sim_hour: 0.0,
        };

        let changed = delta.prune_removed_base(3);

        assert!(changed);
        assert_eq!(delta.removed_base, vec![0, 2]);
    }

    #[test]
    fn bounded_catch_up_window_caps_target_hour() {
        let window = bounded_catch_up_window(10.5, 900.0, MAX_CATCH_UP_HOURS);

        assert!(window.clamped);
        assert_eq!(window.target_hour, 510.5);
        assert_eq!(window.missed_boundaries, 500);
    }

    #[test]
    fn advance_delta_plant_growth_is_time_based_and_monotonic() {
        let registry = test_registry();
        let mut plant = DeltaPlant {
            position: Vec3::ZERO,
            rotation: 0.0,
            height: 8.0,
            species_index: 0,
            stage: GrowthStage::Seedling,
            born_hour: 100.0,
        };

        assert!(!advance_delta_plant_growth(&mut plant, 147.9, &registry));
        assert_eq!(plant.stage, GrowthStage::Seedling);

        assert!(advance_delta_plant_growth(&mut plant, 148.0, &registry));
        assert_eq!(plant.stage, GrowthStage::Young);

        assert!(advance_delta_plant_growth(&mut plant, 244.0, &registry));
        assert_eq!(plant.stage, GrowthStage::Mature);

        assert!(!advance_delta_plant_growth(&mut plant, 400.0, &registry));
        assert_eq!(plant.stage, GrowthStage::Mature);
    }
}
