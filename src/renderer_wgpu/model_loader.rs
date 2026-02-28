use anyhow::{Context, Result};
use glam::Vec3;

use super::geometry::Vertex;
use super::instancing::{upload_prototype, PrototypeMesh};

/// Load a GLB model from bytes and convert it to a `PrototypeMesh`.
///
/// Extracts the first mesh's first primitive. Reads POSITION, NORMAL (or computes
/// flat normals), and COLOR_0 (or falls back to white). Produces the same `Vertex`
/// format used by the instanced shader.
pub fn load_glb(device: &wgpu::Device, bytes: &[u8], label: &str) -> Result<PrototypeMesh> {
    let (document, buffers, _images) = gltf::import_slice(bytes).context("failed to parse GLB")?;

    let mesh = document.meshes().next().context("GLB contains no meshes")?;

    let primitive = mesh
        .primitives()
        .next()
        .context("mesh contains no primitives")?;

    let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

    // Positions (required)
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .context("mesh has no POSITION attribute")?
        .collect();

    // Indices (required for our pipeline)
    let indices: Vec<u32> = reader
        .read_indices()
        .context("mesh has no indices")?
        .into_u32()
        .collect();

    // Normals (optional — compute flat normals from faces if missing)
    let normals: Vec<[f32; 3]> = match reader.read_normals() {
        Some(iter) => iter.collect(),
        None => compute_flat_normals(&positions, &indices),
    };

    // Vertex colors (optional — fall back to white)
    let colors: Vec<[f32; 3]> = match reader.read_colors(0) {
        Some(iter) => iter.into_rgb_f32().collect(),
        None => vec![[1.0, 1.0, 1.0]; positions.len()],
    };

    let vertices: Vec<Vertex> = positions
        .iter()
        .zip(normals.iter())
        .zip(colors.iter())
        .map(|((pos, norm), col)| Vertex {
            position: *pos,
            normal: *norm,
            color: *col,
        })
        .collect();

    Ok(upload_prototype(device, &vertices, &indices, label))
}

/// Compute per-vertex flat normals by averaging face normals for each vertex.
fn compute_flat_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut normals = vec![Vec3::ZERO; positions.len()];

    for tri in indices.chunks_exact(3) {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let a = Vec3::from(positions[i0]);
        let b = Vec3::from(positions[i1]);
        let c = Vec3::from(positions[i2]);
        let face_normal = (b - a).cross(c - a).normalize_or_zero();
        normals[i0] += face_normal;
        normals[i1] += face_normal;
        normals[i2] += face_normal;
    }

    normals
        .into_iter()
        .map(|n| n.normalize_or_zero().to_array())
        .collect()
}

/// Try to load a GLB from `assets/models/{name}.glb`. Returns `None` if the file
/// doesn't exist (allowing fallback to procedural meshes).
pub fn try_load_model(device: &wgpu::Device, name: &str) -> Option<PrototypeMesh> {
    let path = format!("assets/models/{name}.glb");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            log::info!("No model file at {path}, using procedural fallback");
            return None;
        }
    };
    match load_glb(device, &bytes, name) {
        Ok(mesh) => {
            log::info!("Loaded model from {path}");
            Some(mesh)
        }
        Err(e) => {
            log::warn!("Failed to load {path}: {e:#}");
            None
        }
    }
}
