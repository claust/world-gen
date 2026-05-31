use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Growth stage of a plant. Drives render scale and the analytic growth tick.
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

/// Overlay of modifications to a chunk's deterministic plant content. Legacy
/// persistence type from the loaded-only model — still loaded/saved by the delta
/// store for continuity until spread persistence lands (M4), but no longer drives
/// the global PlantWorld sim.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ChunkDelta {
    pub removed_base: Vec<usize>,
    pub added_plants: Vec<DeltaPlant>,
}

impl ChunkDelta {
    pub fn is_empty(&self) -> bool {
        self.removed_base.is_empty() && self.added_plants.is_empty()
    }

    /// Fold another chunk's delta into this one. Used when canonicalizing the
    /// keys of a save written before delta state was keyed by canonical chunk:
    /// two legacy raw chunk ids a whole number of world laps apart collapse to
    /// the same canonical chunk and must be combined. `removed_base` indices and
    /// `added_plants` refer to the shared canonical chunk, so they union/concat;
    /// added-plant positions are normalized into the loaded chunk's span on load.
    pub fn merge(&mut self, other: ChunkDelta) {
        self.removed_base.extend(other.removed_base);
        self.removed_base.sort_unstable();
        self.removed_base.dedup();
        self.added_plants.extend(other.added_plants);
    }

    pub fn prune_removed_base(&mut self, base_len: usize) -> bool {
        let original = self.removed_base.clone();
        self.removed_base.retain(|&index| index < base_len);
        self.removed_base.sort_unstable();
        self.removed_base.dedup();
        self.removed_base != original
    }
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
    use super::ChunkDelta;

    #[test]
    fn prune_removed_base_discards_stale_indices() {
        let mut delta = ChunkDelta {
            removed_base: vec![0, 2, 2, 9],
            added_plants: Vec::new(),
        };

        let changed = delta.prune_removed_base(3);

        assert!(changed);
        assert_eq!(delta.removed_base, vec![0, 2]);
    }
}
