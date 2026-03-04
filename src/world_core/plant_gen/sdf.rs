use fast_surface_nets::ndshape::{RuntimeShape, Shape};
use fast_surface_nets::{surface_nets, SurfaceNetsBuffer};
use glam::Vec3;

use super::config::Hsl;
use super::tree::FoliageBlob;
use super::PlantVertex;
use crate::world_core::color::hsl_to_linear;

fn sdf_sphere(p: Vec3, center: Vec3, r: f32) -> f32 {
    p.distance(center) - r
}

fn smooth_min(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    let m = a * h + b * (1.0 - h);
    m - k * h * (1.0 - h)
}

fn eval_sdf(p: Vec3, blobs: &[FoliageBlob], k: f32) -> f32 {
    let mut d = f32::MAX;
    for blob in blobs {
        let sd = sdf_sphere(p, blob.center, blob.radius);
        d = smooth_min(d, sd, k);
    }
    d
}

pub fn extract_foliage_surface(blobs: &[FoliageBlob], leaf: &Hsl) -> (Vec<PlantVertex>, Vec<u32>) {
    if blobs.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Compute AABB of all blobs
    let mut aabb_min = Vec3::splat(f32::MAX);
    let mut aabb_max = Vec3::splat(f32::MIN);
    let mut radius_sum = 0.0f32;
    let mut min_radius = f32::MAX;
    for blob in blobs {
        let r = blob.radius;
        aabb_min = aabb_min.min(blob.center - Vec3::splat(r));
        aabb_max = aabb_max.max(blob.center + Vec3::splat(r));
        radius_sum += r;
        min_radius = min_radius.min(r);
    }
    let mean_radius = radius_sum / blobs.len() as f32;
    let k = mean_radius * 0.6;

    // Cell size: half the smallest blob radius, clamped so grid stays 8-48 per axis
    let extent = aabb_max - aabb_min;
    let max_extent = extent.x.max(extent.y).max(extent.z);
    let cell_size_desired = min_radius * 0.5;
    let cell_size = cell_size_desired
        .max(max_extent / 48.0)
        .min(max_extent / 8.0);

    // Pad by 1 cell on each side
    let grid_min = aabb_min - Vec3::splat(cell_size);
    let grid_max = aabb_max + Vec3::splat(cell_size);
    let grid_extent = grid_max - grid_min;

    let nx = ((grid_extent.x / cell_size).ceil() as u32 + 1).max(4);
    let ny = ((grid_extent.y / cell_size).ceil() as u32 + 1).max(4);
    let nz = ((grid_extent.z / cell_size).ceil() as u32 + 1).max(4);

    let shape = RuntimeShape::<u32, 3>::new([nx, ny, nz]);
    let mut sdf_values = vec![1.0f32; shape.usize()];

    // Fill SDF grid
    for zi in 0..nz {
        for yi in 0..ny {
            for xi in 0..nx {
                let world_pos = grid_min + Vec3::new(xi as f32, yi as f32, zi as f32) * cell_size;
                let idx = shape.linearize([xi, yi, zi]) as usize;
                sdf_values[idx] = eval_sdf(world_pos, blobs, k);
            }
        }
    }

    // Extract surface
    let mut buffer = SurfaceNetsBuffer::default();
    surface_nets(
        &sdf_values,
        &shape,
        [0; 3],
        [nx - 1, ny - 1, nz - 1],
        &mut buffer,
    );

    if buffer.positions.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Post-process: convert grid-space → world-space, compute normals + colors
    let inv_2h = 1.0 / (2.0 * cell_size);
    let vertices: Vec<PlantVertex> = buffer
        .positions
        .iter()
        .map(|grid_pos| {
            let world_pos = grid_min + Vec3::from_array(*grid_pos) * cell_size;

            // SDF gradient via central differences for smoother normals
            let normal = sdf_gradient(world_pos, blobs, k, cell_size * 0.5, inv_2h);

            // Blob-weighted color blend
            let color = blend_blob_color(world_pos, blobs, leaf);

            PlantVertex {
                position: world_pos.to_array(),
                normal,
                color,
            }
        })
        .collect();

    (vertices, buffer.indices)
}

fn sdf_gradient(p: Vec3, blobs: &[FoliageBlob], k: f32, eps: f32, inv_2h: f32) -> [f32; 3] {
    let dx = eval_sdf(p + Vec3::X * eps, blobs, k) - eval_sdf(p - Vec3::X * eps, blobs, k);
    let dy = eval_sdf(p + Vec3::Y * eps, blobs, k) - eval_sdf(p - Vec3::Y * eps, blobs, k);
    let dz = eval_sdf(p + Vec3::Z * eps, blobs, k) - eval_sdf(p - Vec3::Z * eps, blobs, k);
    let n = Vec3::new(dx, dy, dz) * inv_2h;
    let len = n.length();
    if len > 1e-8 {
        (n / len).to_array()
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn blend_blob_color(p: Vec3, blobs: &[FoliageBlob], leaf: &Hsl) -> [f32; 3] {
    let mut total_weight = 0.0f32;
    let mut blended_hue_shift = 0.0f32;
    let mut blended_light_shift = 0.0f32;

    for blob in blobs {
        let dist = p.distance(blob.center);
        let w = (1.0 - dist / (blob.radius * 1.5)).max(0.0);
        let w = w * w;
        total_weight += w;
        blended_hue_shift += w * blob.hue_shift;
        blended_light_shift += w * blob.light_shift;
    }

    if total_weight > 1e-8 {
        let inv = 1.0 / total_weight;
        let h = leaf.h + blended_hue_shift * inv;
        let l = (leaf.l + blended_light_shift * inv).clamp(0.15, 0.6);
        hsl_to_linear(h, leaf.s, l)
    } else {
        hsl_to_linear(leaf.h, leaf.s, leaf.l)
    }
}
