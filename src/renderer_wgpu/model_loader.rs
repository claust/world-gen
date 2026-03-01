use anyhow::{ensure, Context, Result};
use glam::Vec3;

use super::geometry::Vertex;
use super::instancing::{upload_prototype, PrototypeMesh};

/// Load a GLB model from bytes and convert it to a `PrototypeMesh`.
///
/// Extracts the first mesh's first primitive (must be a triangle list).
/// Reads POSITION, NORMAL (or computes smooth normals), and COLOR_0
/// (or falls back to white). Produces the same `Vertex` format used by
/// the instanced shader.
pub fn load_glb(device: &wgpu::Device, bytes: &[u8], label: &str) -> Result<PrototypeMesh> {
    // Parse document and load only buffer data — skip image decoding.
    let gltf = gltf::Gltf::from_slice(bytes).context("failed to parse GLB")?;
    let buffers =
        gltf::import_buffers(&gltf.document, None, gltf.blob).context("failed to load buffers")?;

    let mesh = gltf
        .document
        .meshes()
        .next()
        .context("GLB contains no meshes")?;

    let primitive = mesh
        .primitives()
        .next()
        .context("mesh contains no primitives")?;

    // Validate primitive mode — we only support triangle lists.
    ensure!(
        primitive.mode() == gltf::mesh::Mode::Triangles,
        "unsupported primitive mode {:?}, expected Triangles",
        primitive.mode()
    );

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

    ensure!(
        indices.len().is_multiple_of(3),
        "index count {} is not a multiple of 3",
        indices.len()
    );

    let vertex_count = positions.len();

    // Normals (optional — compute smooth normals from faces if missing)
    let normals: Vec<[f32; 3]> = match reader.read_normals() {
        Some(iter) => {
            let n: Vec<_> = iter.collect();
            ensure!(
                n.len() == vertex_count,
                "normal count ({}) does not match position count ({vertex_count})",
                n.len()
            );
            n
        }
        None => compute_smooth_normals(&positions, &indices),
    };

    // Vertex colors (optional — fall back to white)
    let colors: Vec<[f32; 3]> = match reader.read_colors(0) {
        Some(iter) => {
            let c: Vec<_> = iter.into_rgb_f32().collect();
            ensure!(
                c.len() == vertex_count,
                "color count ({}) does not match position count ({vertex_count})",
                c.len()
            );
            c
        }
        None => vec![[1.0, 1.0, 1.0]; vertex_count],
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

/// Compute per-vertex smooth normals by averaging face normals at each vertex.
///
/// Each face contributes its area-weighted normal to every vertex it touches,
/// then the accumulated normals are normalized. This produces smooth shading
/// across shared edges. Degenerate faces (zero-area) are skipped via
/// `normalize_or_zero`.
fn compute_smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    debug_assert!(
        indices.len().is_multiple_of(3),
        "index count must be a multiple of 3"
    );

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
/// doesn't exist (allowing fallback to procedural meshes). Other IO errors are
/// logged as warnings.
#[cfg(not(target_arch = "wasm32"))]
pub fn try_load_model(device: &wgpu::Device, name: &str) -> Option<PrototypeMesh> {
    let path = format!("assets/models/{name}.glb");
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::info!("No model file at {path}, using procedural fallback");
            return None;
        }
        Err(e) => {
            log::warn!("Failed to read {path}: {e}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_normals_single_triangle() {
        // Triangle in the XY plane, normal should point +Z
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let indices = [0, 1, 2];

        let normals = compute_smooth_normals(&positions, &indices);
        assert_eq!(normals.len(), 3);
        for n in &normals {
            assert!((n[0]).abs() < 1e-5, "x should be ~0");
            assert!((n[1]).abs() < 1e-5, "y should be ~0");
            assert!((n[2] - 1.0).abs() < 1e-5, "z should be ~1");
        }
    }

    #[test]
    fn smooth_normals_two_shared_triangles() {
        // Two triangles sharing an edge — the shared vertices get averaged normals.
        //   v2 (0,1,0)
        //   /\
        //  /  \
        // v0---v1 (1,0,0)
        //  \  /
        //   \/
        //   v3 (0,-1,0) but offset in Z
        let positions = [
            [0.0, 0.0, 0.0],  // v0
            [1.0, 0.0, 0.0],  // v1
            [0.0, 1.0, 0.0],  // v2
            [0.5, 0.0, -1.0], // v3 (below, pulled back in Z)
        ];
        let indices = [0, 1, 2, 0, 3, 1];

        let normals = compute_smooth_normals(&positions, &indices);
        assert_eq!(normals.len(), 4);

        // v0 and v1 are shared — their normals should be the average of both face normals
        // v2 only belongs to the first face, v3 only to the second
        // All normals should be unit length
        for n in &normals {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!(
                (len - 1.0).abs() < 1e-5,
                "normal should be unit length, got {len}"
            );
        }
    }

    #[test]
    fn smooth_normals_degenerate_triangle() {
        // Degenerate triangle (all vertices collinear) should produce zero normals
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
        let indices = [0, 1, 2];

        let normals = compute_smooth_normals(&positions, &indices);
        assert_eq!(normals.len(), 3);
        for n in &normals {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!(len < 1e-5, "degenerate triangle should produce zero normal");
        }
    }

    #[test]
    fn smooth_normals_empty() {
        let normals = compute_smooth_normals(&[], &[]);
        assert!(normals.is_empty());
    }
}
