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
/// Default panel width in logical pixels (must match PlantEditorPanel::default_width).
const PANEL_WIDTH_PX: f32 = 400.0;
const ORBIT_SPEED: f32 = 1.5;
/// Slow auto-orbit speed (radians/sec) when idle.
const AUTO_ORBIT_SPEED: f32 = 0.15;
/// Mouse drag sensitivity for horizontal orbit (radians per pixel).
const MOUSE_ORBIT_SENSITIVITY: f32 = 0.005;
/// Mouse drag sensitivity for vertical height adjustment (units per pixel).
const MOUSE_HEIGHT_SENSITIVITY: f32 = 0.05;
const MIN_ORBIT_HEIGHT: f32 = 1.0;
const MAX_ORBIT_HEIGHT: f32 = 25.0;

pub struct PlantEditorState {
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
    /// Is left mouse button held in viewport for drag-orbit.
    pub mouse_dragging: bool,
    /// Last known cursor position for computing drag deltas.
    pub last_cursor_pos: Option<(f64, f64)>,
    /// Current camera height (adjustable via vertical mouse drag).
    pub orbit_height: f32,
}

impl PlantEditorState {
    pub fn new(device: &wgpu::Device, seed: u32, species: &SpeciesConfig) -> Self {
        let base_species = species.clone();

        let (ground_mesh, ground_instance) = create_ground_plane(device);

        Self {
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
            mouse_dragging: false,
            last_cursor_pos: None,
            orbit_height: ORBIT_HEIGHT,
        }
    }

    pub fn current_species(&self, params: &PlantParams) -> SpeciesConfig {
        merge_params(&self.base_species, params)
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
                self.orbit_angle += ORBIT_SPEED * dt;
            }
            if self.orbit_right {
                self.orbit_angle -= ORBIT_SPEED * dt;
            }
        }
    }

    /// Stop auto-orbit (called when user sends a debug API camera command).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stop_auto_orbit(&mut self) {
        self.auto_orbit = false;
    }

    /// Start mouse drag orbit.
    pub fn on_mouse_press(&mut self) {
        self.mouse_dragging = true;
        self.auto_orbit = false;
    }

    /// End mouse drag orbit.
    pub fn on_mouse_release(&mut self) {
        self.mouse_dragging = false;
    }

    /// Track cursor position and apply drag-orbit when dragging.
    pub fn on_cursor_move(&mut self, x: f64, y: f64) {
        if let Some((last_x, last_y)) = self.last_cursor_pos {
            if self.mouse_dragging {
                let dx = (x - last_x) as f32;
                let dy = (y - last_y) as f32;
                self.orbit_angle -= dx * MOUSE_ORBIT_SENSITIVITY;
                self.orbit_height = (self.orbit_height + dy * MOUSE_HEIGHT_SENSITIVITY)
                    .clamp(MIN_ORBIT_HEIGHT, MAX_ORBIT_HEIGHT);
            }
        }
        self.last_cursor_pos = Some((x, y));
    }

    /// Compute camera position and look direction for the current orbit angle.
    /// Shifts the camera so the plant appears centered in the visible viewport
    /// (accounting for the left editor panel).
    pub fn orbit_camera(
        &self,
        screen_width: f32,
        fov_y: f32,
        aspect: f32,
    ) -> (glam::Vec3, f32, f32) {
        let cos_a = self.orbit_angle.cos();
        let sin_a = self.orbit_angle.sin();

        // The panel covers PANEL_WIDTH_PX of the left side. The visible center
        // is shifted right by panel_frac/2 in screen space, which is panel_frac
        // in NDC (since NDC width is 2). Convert to world-space offset at the
        // orbit distance.
        let panel_frac = (PANEL_WIDTH_PX / screen_width).min(0.5);
        let offset = panel_frac * ORBIT_DISTANCE * (fov_y / 2.0).tan() * aspect;

        // Shift camera to the left (perpendicular to orbit direction) so the
        // plant projects to the right of screen center, into the visible area.
        let cam_x = sin_a * ORBIT_DISTANCE - cos_a * offset;
        let cam_z = cos_a * ORBIT_DISTANCE + sin_a * offset;
        let cam_pos = glam::Vec3::new(cam_x, self.orbit_height, cam_z);

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

    // Body Plan
    spec.body_plan.kind = params.body_kind.clone();
    spec.body_plan.stem_count = params.stem_count;
    let (h_min, h_max) = min_max_f32(params.max_height_min, params.max_height_max);
    spec.body_plan.max_height = [h_min, h_max];

    // Trunk
    spec.trunk.taper = params.taper;
    spec.trunk.base_flare = params.base_flare;
    spec.trunk.straightness = params.straightness;
    spec.trunk.thickness_ratio = params.thickness_ratio;

    // Crown
    spec.crown.shape = params.crown_shape.clone();
    spec.crown.crown_base = params.crown_base;
    spec.crown.aspect_ratio = params.aspect_ratio;
    spec.crown.density = params.crown_density;
    spec.crown.asymmetry = params.asymmetry;

    // Branching
    spec.branching.apical_dominance = params.apical_dominance;
    spec.branching.gravity_response = params.gravity_response;
    spec.branching.length_profile = params.length_profile.clone();
    spec.branching.max_depth = params.max_depth;
    spec.branching.arrangement.kind = params.arrangement_type.clone();
    spec.branching.arrangement.angle = if params.arrangement_type == "spiral" {
        Some(params.arrangement_angle)
    } else {
        None
    };
    let (bpn_min, bpn_max) =
        min_max_u32(params.branches_per_node_min, params.branches_per_node_max);
    spec.branching.branches_per_node = [bpn_min, bpn_max];
    let (iab_min, iab_max) = min_max_f32(
        params.insertion_angle_base_min,
        params.insertion_angle_base_max,
    );
    spec.branching.insertion_angle.base = [iab_min, iab_max];
    let (iat_min, iat_max) = min_max_f32(
        params.insertion_angle_tip_min,
        params.insertion_angle_tip_max,
    );
    spec.branching.insertion_angle.tip = [iat_min, iat_max];
    spec.branching.child_length_ratio = params.child_length_ratio;
    spec.branching.child_thickness_ratio = params.child_thickness_ratio;
    spec.branching.randomness = params.randomness;

    // Foliage
    spec.foliage.style = params.foliage_style.clone();
    let (ls_min, ls_max) = min_max_f32(params.leaf_size_min, params.leaf_size_max);
    spec.foliage.leaf_size = [ls_min, ls_max];
    spec.foliage.cluster_strategy.kind = params.cluster_type.clone();
    spec.foliage.cluster_strategy.count =
        if params.cluster_type == "clusters" || params.cluster_type == "ring" {
            Some(params.cluster_count)
        } else {
            None
        };
    spec.foliage.droop = params.droop;
    spec.foliage.coverage = params.coverage;

    // Color
    spec.color.bark.h = params.bark_h;
    spec.color.bark.s = params.bark_s;
    spec.color.bark.l = params.bark_l;
    spec.color.leaf.h = params.leaf_h;
    spec.color.leaf.s = params.leaf_s;
    spec.color.leaf.l = params.leaf_l;
    spec.color.leaf_variance = Some(params.leaf_variance);

    spec
}

fn min_max_f32(a: f32, b: f32) -> (f32, f32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
}

fn min_max_u32(a: u32, b: u32) -> (u32, u32) {
    if a <= b {
        (a, b)
    } else {
        (b, a)
    }
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
