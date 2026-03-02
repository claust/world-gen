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
    pub style: String,
    pub leaf_size: [f32; 2],
    pub cluster_strategy: ClusterStrategy,
    pub droop: f32,
    pub coverage: f32,
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
