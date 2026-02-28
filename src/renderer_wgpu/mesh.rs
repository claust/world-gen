use glam::Vec3;

use super::geometry::{append_box, append_octahedron, append_quad, append_triangle, Vertex};
use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    ChunkTerrain, HouseInstance, TreeInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};

pub struct CpuChunkMesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

pub fn build_terrain_mesh(chunk: &ChunkTerrain, biome_map: &BiomeMap) -> Option<CpuChunkMesh> {
    let side = CHUNK_GRID_RESOLUTION;
    let total = side * side;
    if chunk.heights.len() != total || biome_map.values.len() != total {
        return None;
    }
    if chunk.max_height < chunk.min_height {
        return None;
    }

    let cell_size = CHUNK_SIZE_METERS / (side - 1) as f32;
    let origin_x = chunk.coord.x as f32 * CHUNK_SIZE_METERS;
    let origin_z = chunk.coord.y as f32 * CHUNK_SIZE_METERS;

    let normals: Vec<Vec3> = (0..total)
        .map(|idx| {
            let x = idx % side;
            let z = idx / side;

            let x0 = x.saturating_sub(1);
            let x1 = (x + 1).min(side - 1);
            let z0 = z.saturating_sub(1);
            let z1 = (z + 1).min(side - 1);

            let h_l = chunk.heights[z * side + x0];
            let h_r = chunk.heights[z * side + x1];
            let h_d = chunk.heights[z0 * side + x];
            let h_u = chunk.heights[z1 * side + x];

            Vec3::new(h_l - h_r, cell_size * 2.0, h_d - h_u).normalize()
        })
        .collect();

    let vertices: Vec<Vertex> = (0..total)
        .map(|idx| {
            let x = idx % side;
            let z = idx / side;
            let world_x = origin_x + x as f32 * cell_size;
            let world_z = origin_z + z as f32 * cell_size;
            let h = chunk.heights[idx];
            let biome = biome_map.values[idx];
            let color = biome_ground_color(biome, h);
            let n = normals[idx];
            Vertex {
                position: [world_x, h, world_z],
                normal: [n.x, n.y, n.z],
                color: [color.x, color.y, color.z],
            }
        })
        .collect();

    let mut indices = Vec::with_capacity((side - 1) * (side - 1) * 6);
    for z in 0..(side - 1) {
        for x in 0..(side - 1) {
            let i0 = (z * side + x) as u32;
            let i1 = i0 + 1;
            let i2 = i0 + side as u32;
            let i3 = i2 + 1;
            indices.extend_from_slice(&[i0, i2, i1, i1, i2, i3]);
        }
    }

    Some(CpuChunkMesh { vertices, indices })
}

pub fn build_tree_mesh(trees: &[TreeInstance]) -> Option<CpuChunkMesh> {
    if trees.is_empty() {
        return None;
    }

    let mut vertices = Vec::with_capacity(trees.len() * 12);
    let mut indices = Vec::with_capacity(trees.len() * 54);

    for tree in trees {
        let trunk_center = tree.position + Vec3::new(0.0, tree.trunk_height * 0.5, 0.0);
        append_box(
            &mut vertices,
            &mut indices,
            trunk_center,
            Vec3::new(0.30, tree.trunk_height * 0.5, 0.30),
            Vec3::new(0.33, 0.22, 0.11),
        );

        let canopy_center =
            tree.position + Vec3::new(0.0, tree.trunk_height + tree.canopy_radius, 0.0);
        append_octahedron(
            &mut vertices,
            &mut indices,
            canopy_center,
            tree.canopy_radius,
            Vec3::new(0.14, 0.38, 0.16),
        );
    }

    Some(CpuChunkMesh { vertices, indices })
}

pub fn build_house_mesh(houses: &[HouseInstance]) -> Option<CpuChunkMesh> {
    if houses.is_empty() {
        return None;
    }

    let mut vertices = Vec::with_capacity(houses.len() * 48);
    let mut indices = Vec::with_capacity(houses.len() * 72);

    let wall_color = Vec3::new(0.72, 0.63, 0.46);
    let roof_color = Vec3::new(0.55, 0.22, 0.15);

    let half_w = 2.5;
    let half_d = 2.0;
    let wall_h = 3.0;
    let roof_h = 2.0;

    for house in houses {
        let cos_r = house.rotation.cos();
        let sin_r = house.rotation.sin();

        let rot = |lx: f32, lz: f32| -> Vec3 {
            Vec3::new(lx * cos_r - lz * sin_r, 0.0, lx * sin_r + lz * cos_r)
        };

        let base = house.position;

        let bl = base + rot(-half_w, -half_d);
        let br = base + rot(half_w, -half_d);
        let fr = base + rot(half_w, half_d);
        let fl = base + rot(-half_w, half_d);

        let tbl = bl + Vec3::Y * wall_h;
        let tbr = br + Vec3::Y * wall_h;
        let tfr = fr + Vec3::Y * wall_h;
        let tfl = fl + Vec3::Y * wall_h;

        let ridge_l = base + rot(-half_w, 0.0) + Vec3::Y * (wall_h + roof_h);
        let ridge_r = base + rot(half_w, 0.0) + Vec3::Y * (wall_h + roof_h);

        // Walls
        let n_front = rot(0.0, 1.0);
        append_quad(
            &mut vertices,
            &mut indices,
            [fl, fr, tfr, tfl],
            n_front,
            wall_color,
        );

        let n_back = rot(0.0, -1.0);
        append_quad(
            &mut vertices,
            &mut indices,
            [br, bl, tbl, tbr],
            n_back,
            wall_color,
        );

        let n_right = rot(1.0, 0.0);
        append_quad(
            &mut vertices,
            &mut indices,
            [fr, br, tbr, tfr],
            n_right,
            wall_color,
        );

        let n_left = rot(-1.0, 0.0);
        append_quad(
            &mut vertices,
            &mut indices,
            [bl, fl, tfl, tbl],
            n_left,
            wall_color,
        );

        // Roof slopes
        let roof_n_front = rot(0.0, 1.0) * half_d + Vec3::Y * roof_h;
        let roof_n_front = roof_n_front.normalize_or_zero();
        append_quad(
            &mut vertices,
            &mut indices,
            [tfl, tfr, ridge_r, ridge_l],
            roof_n_front,
            roof_color,
        );

        let roof_n_back = rot(0.0, -1.0) * half_d + Vec3::Y * roof_h;
        let roof_n_back = roof_n_back.normalize_or_zero();
        append_quad(
            &mut vertices,
            &mut indices,
            [tbr, tbl, ridge_l, ridge_r],
            roof_n_back,
            roof_color,
        );

        // Gable ends
        append_triangle(&mut vertices, &mut indices, tbl, tfl, ridge_l, roof_color);
        append_triangle(&mut vertices, &mut indices, tfr, tbr, ridge_r, roof_color);
    }

    Some(CpuChunkMesh { vertices, indices })
}

fn biome_ground_color(biome: Biome, height: f32) -> Vec3 {
    let base = match biome {
        Biome::Snow => Vec3::new(0.90, 0.92, 0.95),
        Biome::Rock => Vec3::new(0.46, 0.48, 0.50),
        Biome::Desert => Vec3::new(0.70, 0.60, 0.36),
        Biome::Forest => Vec3::new(0.21, 0.43, 0.23),
        Biome::Grassland => Vec3::new(0.34, 0.52, 0.24),
    };

    let tint = ((height + 40.0) / 260.0).clamp(0.0, 1.0);
    base.lerp(Vec3::splat(0.75), tint * 0.08)
}
