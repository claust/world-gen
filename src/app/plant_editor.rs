use std::sync::mpsc;

use serde_json::Value;

use crate::renderer_wgpu::geometry::Vertex;
use crate::renderer_wgpu::instancing::{
    upload_instances, upload_prototype, GpuInstanceChunk, InstanceData, PrototypeMesh,
};
use crate::renderer_wgpu::model_loader;
use crate::ui::plant_editor_panel::PlantParams;

/// Orbit camera constants for the plant editor.
const ORBIT_TARGET_Y: f32 = 7.0;
const ORBIT_DISTANCE: f32 = 28.0;
const ORBIT_HEIGHT: f32 = 10.0;
/// Horizontal offset to compensate for the left egui panel.
const ORBIT_PANEL_OFFSET: f32 = 3.0;
const ORBIT_SPEED: f32 = 1.5;
/// Slow auto-orbit speed (radians/sec) when idle.
const AUTO_ORBIT_SPEED: f32 = 0.15;

pub struct PlantEditorState {
    base_species: Value,
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
    pub fn new(device: &wgpu::Device) -> Self {
        let base_species: Value =
            serde_json::from_str(include_str!("../../tools/plant-gen/examples/oak.json"))
                .expect("invalid oak.json");

        let (ground_mesh, ground_instance) = create_ground_plane(device);

        Self {
            base_species,
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

    pub fn request_generation(&mut self, params: &PlantParams) {
        let species = merge_params(&self.base_species, params);
        self.generator.request(species);
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

    pub fn load_glb_result(&mut self, device: &wgpu::Device, glb_bytes: &[u8]) {
        match model_loader::load_glb(device, glb_bytes, "plant-editor-tree") {
            Ok(mesh) => self.set_tree_mesh(device, mesh),
            Err(e) => log::warn!("failed to load generated plant GLB: {e:#}"),
        }
    }
}

pub struct MeshGenerator {
    tx: mpsc::Sender<Vec<u8>>,
    rx: mpsc::Receiver<Vec<u8>>,
    busy: bool,
}

impl MeshGenerator {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            busy: false,
        }
    }

    pub fn request(&mut self, species_json: Value) {
        self.busy = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || match generate_glb(species_json) {
            Ok(bytes) => {
                let _ = tx.send(bytes);
            }
            Err(e) => {
                log::warn!("plant generation failed: {e}");
            }
        });
    }

    pub fn poll(&mut self) -> Option<Vec<u8>> {
        match self.rx.try_recv() {
            Ok(bytes) => {
                self.busy = false;
                Some(bytes)
            }
            Err(_) => None,
        }
    }

    pub fn is_busy(&self) -> bool {
        self.busy
    }
}

fn generate_glb(species: Value) -> anyhow::Result<Vec<u8>> {
    let tmp_dir = std::env::temp_dir();
    let json_path = tmp_dir.join("plant_editor_species.json");
    let glb_path = tmp_dir.join("plant_editor_output.glb");

    std::fs::write(&json_path, serde_json::to_string_pretty(&species)?)?;

    let output = std::process::Command::new("bun")
        .arg("tools/plant-gen/render.ts")
        .arg(json_path.to_str().unwrap())
        .arg(glb_path.to_str().unwrap())
        .arg("--format")
        .arg("glb")
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("bun render.ts failed: {stderr}");
    }

    let bytes = std::fs::read(&glb_path)?;
    Ok(bytes)
}

fn merge_params(base: &Value, params: &PlantParams) -> Value {
    let mut spec = base.clone();

    if let Some(crown) = spec.get_mut("crown").and_then(|v| v.as_object_mut()) {
        crown.insert(
            "shape".to_string(),
            Value::String(params.crown_shape.clone()),
        );
        crown.insert(
            "crown_base".to_string(),
            serde_json::json!(params.crown_base),
        );
        crown.insert(
            "aspect_ratio".to_string(),
            serde_json::json!(params.aspect_ratio),
        );
        crown.insert(
            "density".to_string(),
            serde_json::json!(params.crown_density),
        );
    }

    if let Some(branching) = spec.get_mut("branching").and_then(|v| v.as_object_mut()) {
        branching.insert(
            "apical_dominance".to_string(),
            serde_json::json!(params.apical_dominance),
        );
        branching.insert(
            "gravity_response".to_string(),
            serde_json::json!(params.gravity_response),
        );
        branching.insert(
            "length_profile".to_string(),
            Value::String(params.length_profile.clone()),
        );
    }

    if let Some(foliage) = spec.get_mut("foliage").and_then(|v| v.as_object_mut()) {
        foliage.insert(
            "style".to_string(),
            Value::String(params.foliage_style.clone()),
        );
    }

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
