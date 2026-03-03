use glam::{IVec2, Mat4, Vec3};

use crate::world_core::chunk::CHUNK_SIZE_METERS;

// Vertical bounds for chunk AABBs. These must fully enclose the terrain height range.
// Use conservative values to avoid incorrectly culling low or high terrain.
const MIN_Y: f32 = -256.0;
const MAX_Y: f32 = 1024.0;

pub struct Frustum {
    planes: [[f32; 4]; 6],
}

impl Frustum {
    /// Extract 6 clip planes from a view-projection matrix (Gribb/Hartmann method).
    pub fn from_view_proj(vp: Mat4) -> Self {
        let r0 = vp.row(0);
        let r1 = vp.row(1);
        let r2 = vp.row(2);
        let r3 = vp.row(3);

        let raw = [
            r3 + r0, // left
            r3 - r0, // right
            r3 + r1, // bottom
            r3 - r1, // top
            r3 + r2, // near
            r3 - r2, // far
        ];

        let mut planes = [[0.0f32; 4]; 6];
        for (i, p) in raw.iter().enumerate() {
            let len = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt();
            if len > 0.0 {
                planes[i] = [p.x / len, p.y / len, p.z / len, p.w / len];
            }
        }

        Self { planes }
    }

    /// Returns true if the AABB is at least partially inside the frustum.
    pub fn is_aabb_visible(&self, min: Vec3, max: Vec3) -> bool {
        for plane in &self.planes {
            let (a, b, c, d) = (plane[0], plane[1], plane[2], plane[3]);

            // Find the corner most in the direction of the plane normal (p-vertex)
            let px = if a >= 0.0 { max.x } else { min.x };
            let py = if b >= 0.0 { max.y } else { min.y };
            let pz = if c >= 0.0 { max.z } else { min.z };

            // If the p-vertex is outside, the entire AABB is outside this plane
            if a * px + b * py + c * pz + d < 0.0 {
                return false;
            }
        }
        true
    }

    /// Test visibility for a chunk at the given grid coordinate.
    pub fn is_chunk_visible(&self, coord: IVec2) -> bool {
        let min = Vec3::new(
            coord.x as f32 * CHUNK_SIZE_METERS,
            MIN_Y,
            coord.y as f32 * CHUNK_SIZE_METERS,
        );
        let max = Vec3::new(min.x + CHUNK_SIZE_METERS, MAX_Y, min.z + CHUNK_SIZE_METERS);
        self.is_aabb_visible(min, max)
    }
}
