use crate::renderer_wgpu::geometry::Vertex;
use crate::renderer_wgpu::instancing::{
    upload_instances, upload_prototype, GpuInstanceChunk, InstanceData, PrototypeMesh,
};
use crate::ui::plant_editor_panel::PlantParams;
use crate::world_core::plant_gen::config::SpeciesConfig;
use crate::world_core::plant_gen::{generate_plant_mesh, PlantMesh};

/// Orbit camera constants for the plant editor.
const ORBIT_TARGET_Y: f32 = 7.0;
const ORBIT_DISTANCE: f32 = 28.0;
const ORBIT_HEIGHT: f32 = 10.0;
/// Horizontal offset to compensate for the left egui panel.
const ORBIT_PANEL_OFFSET: f32 = 3.0;
const ORBIT_SPEED: f32 = 1.5;
/// Slow auto-orbit speed (radians/sec) when idle.
const AUTO_ORBIT_SPEED: f32 = 0.15;

const SPECIES_PRESETS: &[(&str, &str)] = &[
    (
        "Oak",
        include_str!("../world_core/plant_gen/species/oak.json"),
    ),
    (
        "Birch",
        include_str!("../world_core/plant_gen/species/birch.json"),
    ),
    (
        "Acacia",
        include_str!("../world_core/plant_gen/species/acacia.json"),
    ),
    (
        "Palm",
        include_str!("../world_core/plant_gen/species/palm.json"),
    ),
    (
        "Shrub",
        include_str!("../world_core/plant_gen/species/shrub.json"),
    ),
    (
        "Spruce",
        include_str!("../world_core/plant_gen/species/spruce.json"),
    ),
    (
        "Willow",
        include_str!("../world_core/plant_gen/species/willow.json"),
    ),
];

pub struct PlantEditorState {
    all_species: Vec<(String, SpeciesConfig)>,
    base_species: SpeciesConfig,
    seed: u32,
    pub tree_mesh: Option<PrototypeMesh>,
    pub tree_instance: Option<GpuInstanceChunk>,
    pub ground_mesh: PrototypeMesh,
    pub ground_instance: GpuInstanceChunk,
    pub generator: MeshGenerator,
    /// Current orbit angle (radians) around the tree.
    pub orbit_angle: f32,
    pub orbit_left: bool,
    pub orbit_right: bool,
    /// Auto-orbit: camera slowly circles until user interacts.
    pub auto_orbit: bool,
}

impl PlantEditorState {
    pub fn new(device: &wgpu::Device, seed: u32) -> Self {
        let all_species: Vec<(String, SpeciesConfig)> = SPECIES_PRESETS
            .iter()
            .map(|(name, json)| {
                let spec: SpeciesConfig = serde_json::from_str(json)
                    .unwrap_or_else(|e| panic!("invalid {name}.json: {e}"));
                (name.to_string(), spec)
            })
            .collect();
        let base_species = all_species[0].1.clone();

        let (ground_mesh, ground_instance) = create_ground_plane(device);

        Self {
            all_species,
            base_species,
            seed,
            tree_mesh: None,
            tree_instance: None,
            ground_mesh,
            ground_instance,
            generator: MeshGenerator::new(),
            orbit_angle: 0.0,
            orbit_left: false,
            orbit_right: false,
            auto_orbit: true,
        }
    }

    pub fn species_names(&self) -> Vec<String> {
        self.all_species
            .iter()
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// Switch to a different base species. Returns the new PlantParams extracted from it.
    pub fn set_base_species(&mut self, name: &str) -> PlantParams {
        if let Some((_, spec)) = self.all_species.iter().find(|(n, _)| n == name) {
            self.base_species = spec.clone();
        }
        PlantParams::from_species(&self.base_species)
    }

    pub fn request_generation(&mut self, params: &PlantParams) {
        let species = merge_params(&self.base_species, params);
        self.generator.request(species, self.seed);
    }

    pub fn set_tree_mesh(&mut self, device: &wgpu::Device, mesh: PrototypeMesh) {
        let instance_data = [InstanceData {
            position: [0.0, 0.0, 0.0],
            rotation_y: 0.0,
            scale: [1.0, 1.0, 1.0],
            _pad: 0.0,
            color: [1.0, 1.0, 1.0, 1.0],
        }];
        let instance = upload_instances(device, &instance_data, "plant-editor-tree");
        self.tree_mesh = Some(mesh);
        self.tree_instance = instance;
    }

    /// Update the orbit angle. Auto-orbits slowly until user presses left/right.
    pub fn update_orbit(&mut self, dt: f32) {
        if self.orbit_left || self.orbit_right {
            self.auto_orbit = false;
        }

        if self.auto_orbit {
            self.orbit_angle += AUTO_ORBIT_SPEED * dt;
        } else {
            if self.orbit_left {
                self.orbit_angle -= ORBIT_SPEED * dt;
            }
            if self.orbit_right {
                self.orbit_angle += ORBIT_SPEED * dt;
            }
        }
    }

    /// Stop auto-orbit (called when user sends a debug API camera command).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stop_auto_orbit(&mut self) {
        self.auto_orbit = false;
    }

    /// Compute camera position and look direction for the current orbit angle.
    pub fn orbit_camera(&self) -> (glam::Vec3, f32, f32) {
        let cos_a = self.orbit_angle.cos();
        let sin_a = self.orbit_angle.sin();

        let cam_x = sin_a * ORBIT_DISTANCE + cos_a * ORBIT_PANEL_OFFSET;
        let cam_z = cos_a * ORBIT_DISTANCE - sin_a * ORBIT_PANEL_OFFSET;
        let cam_pos = glam::Vec3::new(cam_x, ORBIT_HEIGHT, cam_z);

        let target = glam::Vec3::new(0.0, ORBIT_TARGET_Y, 0.0);
        let to_target = target - cam_pos;
        let yaw = to_target.z.atan2(to_target.x);
        let pitch = (to_target.y / to_target.length()).asin();

        (cam_pos, yaw, pitch)
    }

    /// Upload a generated PlantMesh as a GPU prototype.
    pub fn load_plant_mesh(&mut self, device: &wgpu::Device, plant_mesh: &PlantMesh) {
        let vertices: Vec<Vertex> = plant_mesh
            .vertices
            .iter()
            .map(|v| Vertex {
                position: v.position,
                normal: v.normal,
                color: v.color,
            })
            .collect();
        let mesh = upload_prototype(device, &vertices, &plant_mesh.indices, "plant-editor-tree");
        self.set_tree_mesh(device, mesh);
    }
}

pub struct MeshGenerator {
    result: Option<PlantMesh>,
}

impl MeshGenerator {
    pub fn new() -> Self {
        Self { result: None }
    }

    pub fn request(&mut self, species: SpeciesConfig, seed: u32) {
        self.result = Some(generate_plant_mesh(&species, seed));
    }

    pub fn poll(&mut self) -> Option<PlantMesh> {
        self.result.take()
    }
}

fn merge_params(base: &SpeciesConfig, params: &PlantParams) -> SpeciesConfig {
    let mut spec = base.clone();
    spec.crown.shape = params.crown_shape.clone();
    spec.crown.crown_base = params.crown_base;
    spec.crown.aspect_ratio = params.aspect_ratio;
    spec.crown.density = params.crown_density;
    spec.branching.apical_dominance = params.apical_dominance;
    spec.branching.gravity_response = params.gravity_response;
    spec.branching.length_profile = params.length_profile.clone();
    spec.foliage.style = params.foliage_style.clone();
    spec
}

fn create_ground_plane(device: &wgpu::Device) -> (PrototypeMesh, GpuInstanceChunk) {
    let half = 30.0_f32;
    let color = [0.35, 0.42, 0.25];
    let normal = [0.0, 1.0, 0.0];

    let vertices = vec![
        Vertex {
            position: [-half, 0.0, -half],
            normal,
            color,
        },
        Vertex {
            position: [half, 0.0, -half],
            normal,
            color,
        },
        Vertex {
            position: [half, 0.0, half],
            normal,
            color,
        },
        Vertex {
            position: [-half, 0.0, half],
            normal,
            color,
        },
    ];
    let indices = vec![0, 1, 2, 0, 2, 3];

    let mesh = upload_prototype(device, &vertices, &indices, "plant-editor-ground");

    let instance_data = [InstanceData {
        position: [0.0, 0.0, 0.0],
        rotation_y: 0.0,
        scale: [1.0, 1.0, 1.0],
        _pad: 0.0,
        color: [1.0, 1.0, 1.0, 1.0],
    }];
    let instance =
        upload_instances(device, &instance_data, "plant-editor-ground").expect("non-empty");

    (mesh, instance)
}
