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
