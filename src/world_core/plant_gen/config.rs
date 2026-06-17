use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SpeciesConfig {
    pub name: String,
    pub body_plan: BodyPlan,
    pub trunk: Trunk,
    pub branching: Branching,
    pub crown: Crown,
    pub foliage: Foliage,
    pub color: ColorConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BodyPlan {
    pub kind: String,
    pub stem_count: u32,
    pub max_height: [f32; 2],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Trunk {
    pub taper: f32,
    pub base_flare: f32,
    pub straightness: f32,
    pub thickness_ratio: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Branching {
    pub apical_dominance: f32,
    pub max_depth: u32,
    pub arrangement: Arrangement,
    pub branches_per_node: [u32; 2],
    pub insertion_angle: InsertionAngle,
    pub length_profile: String,
    pub child_length_ratio: f32,
    pub child_thickness_ratio: f32,
    pub gravity_response: f32,
    pub randomness: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Arrangement {
    #[serde(rename = "type")]
    pub kind: String,
    pub angle: Option<f32>,
    pub count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InsertionAngle {
    pub base: [f32; 2],
    pub tip: [f32; 2],
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Crown {
    pub shape: String,
    pub crown_base: f32,
    pub aspect_ratio: f32,
    pub density: f32,
    pub asymmetry: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Foliage {
    pub leaf_type: LeafType,
    pub leaf_size: [f32; 2],
    pub cluster_strategy: ClusterStrategy,
    pub droop: f32,
    pub coverage: f32,
}

/// The kind of foliage a species grows. This is a *rendering* trait — it drives
/// how the crown mesh is built (see `plant_gen`), not base-world generation, so
/// it is intentionally excluded from the `generation_key` (see `herbarium`).
///
/// `None` means a leafless skeleton (dead snags, bare reeds); every other
/// variant grows a crown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LeafType {
    None,
    Broadleaf,
    Needle,
    ScaleLeaf,
    PalmFrond,
}

impl LeafType {
    /// Whether this species grows a crown at all.
    pub fn has_foliage(self) -> bool {
        self != LeafType::None
    }

    /// The lowercase label used in species JSON and the plant-editor dropdown.
    pub fn label(self) -> &'static str {
        match self {
            LeafType::None => "none",
            LeafType::Broadleaf => "broadleaf",
            LeafType::Needle => "needle",
            LeafType::ScaleLeaf => "scale_leaf",
            LeafType::PalmFrond => "palm_frond",
        }
    }

    /// Parse a label back into a variant, falling back to `Broadleaf` for any
    /// unrecognised string (the editor only ever feeds known labels).
    pub fn from_label(s: &str) -> LeafType {
        match s {
            "none" => LeafType::None,
            "needle" => LeafType::Needle,
            "scale_leaf" => LeafType::ScaleLeaf,
            "palm_frond" => LeafType::PalmFrond,
            _ => LeafType::Broadleaf,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ClusterStrategy {
    #[serde(rename = "type")]
    pub kind: String,
    pub count: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ColorConfig {
    pub bark: Hsl,
    pub leaf: Hsl,
    pub leaf_variance: Option<f32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Hsl {
    pub h: f32,
    pub s: f32,
    pub l: f32,
}

impl SpeciesConfig {
    /// Return a simplified clone for distant LOD rendering.
    pub fn simplify_for_lod(&self) -> Self {
        let mut c = self.clone();
        c.branching.max_depth = c.branching.max_depth.saturating_sub(1);
        c.branching.branches_per_node = [
            c.branching.branches_per_node[0].min(2),
            c.branching.branches_per_node[1].min(2),
        ];
        c.crown.density *= 0.4;
        c.body_plan.stem_count = c.body_plan.stem_count.min(3);
        c
    }

    /// Derive the leafless snag rendered for `GrowthStage::Dead`: no foliage,
    /// fewer and shorter limbs (twigs rot first), and a crooked, sagging
    /// skeleton. The mesh is bark-only — without foliage there is no SDF pass —
    /// so one snag prototype serves every render distance.
    pub fn deadify(&self) -> Self {
        let mut c = self.clone();
        c.foliage.leaf_type = LeafType::None;
        c.branching.max_depth = c.branching.max_depth.saturating_sub(1).max(1);
        c.branching.branches_per_node = [
            c.branching.branches_per_node[0].saturating_sub(1).max(1),
            c.branching.branches_per_node[1].saturating_sub(1).max(1),
        ];
        c.branching.child_length_ratio *= 0.85;
        c.trunk.straightness *= 0.5;
        c.branching.apical_dominance *= 0.5;
        c.branching.randomness = (c.branching.randomness * 1.5 + 0.1).min(1.0);
        c.branching.gravity_response += 0.4;
        c
    }
}
