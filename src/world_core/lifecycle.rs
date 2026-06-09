use serde::{Deserialize, Serialize};

/// Growth stage of a plant. Drives render scale and the analytic growth tick.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum GrowthStage {
    Seedling,
    Young,
    Mature,
    /// Standing dead snag: leafless, crooked, slightly smaller. Does not
    /// spread; despawns (freeing its ground) after the species' `snag_hours`.
    Dead,
}

impl GrowthStage {
    pub const fn scale_factor(self) -> f32 {
        match self {
            Self::Seedling => 0.15,
            Self::Young => 0.50,
            Self::Mature => 1.0,
            Self::Dead => 0.85,
        }
    }
}

/// Multiplicative tint applied to dead instances so snags read as grey-brown
/// weathered wood regardless of the species' living bark colour. Shared by the
/// world renderer and the plant editor's snag preview.
pub const DEAD_TINT: [f32; 4] = [0.62, 0.54, 0.45, 1.0];
