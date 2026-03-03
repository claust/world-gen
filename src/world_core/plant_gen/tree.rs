use glam::Vec3;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::config::SpeciesConfig;
use super::crown::{is_inside_crown, length_profile};
use crate::world_core::content::sampling::hash4;

#[derive(Clone, Debug)]
pub struct BranchSegment {
    pub start: Vec3,
    pub end: Vec3,
    pub start_radius: f32,
    pub end_radius: f32,
    pub depth: u32,
}

#[derive(Clone, Debug)]
pub struct FoliageBlob {
    pub center: Vec3,
    pub radius: f32,
    pub hue_shift: f32,
    pub light_shift: f32,
}

pub struct TreeData {
    pub segments: Vec<BranchSegment>,
    pub foliage: Vec<FoliageBlob>,
    pub height: f32,
}

/// Remove foliage blobs completely enclosed inside another blob.
/// Blob A is inside blob B when `distance(A.center, B.center) + A.radius <= B.radius`.
pub fn compact_foliage(blobs: &mut Vec<FoliageBlob>) {
    let n = blobs.len();
    if n <= 1 {
        return;
    }
    let mut keep = vec![true; n];
    for i in 0..n {
        if !keep[i] {
            continue;
        }
        for j in 0..n {
            if i == j || !keep[j] {
                continue;
            }
            let dist = blobs[i].center.distance(blobs[j].center);
            if dist + blobs[j].radius <= blobs[i].radius {
                keep[j] = false;
            }
        }
    }
    let mut w = 0;
    for r in 0..n {
        if keep[r] {
            if w != r {
                blobs[w] = blobs[r].clone();
            }
            w += 1;
        }
    }
    blobs.truncate(w);

    // Cap foliage blob count — keep the largest (most visually significant) blobs
    const MAX_FOLIAGE_BLOBS: usize = 100;
    if blobs.len() > MAX_FOLIAGE_BLOBS {
        blobs.sort_by(|a, b| b.radius.partial_cmp(&a.radius).unwrap());
        blobs.truncate(MAX_FOLIAGE_BLOBS);
    }
}

/// Compute a branch direction by tilting `parent_dir` by `insert_angle`, rotated around it by `rot_angle`.
fn branch_dir_3d(parent_dir: Vec3, insert_angle_rad: f32, rot_rad: f32) -> Vec3 {
    let ref_vec = if parent_dir.y.abs() < 0.95 {
        Vec3::Y
    } else {
        Vec3::X
    };
    let p1 = parent_dir.cross(ref_vec).normalize();
    let p2 = parent_dir.cross(p1);
    let rot_perp = p1 * rot_rad.cos() + p2 * rot_rad.sin();
    (parent_dir * insert_angle_rad.cos() + rot_perp * insert_angle_rad.sin()).normalize()
}

pub fn generate_tree(spec: &SpeciesConfig, seed: u32) -> TreeData {
    let mut rng = StdRng::seed_from_u64(hash4(seed, 0x504C_414E, 0x5452_4545, 0) as u64);
    let mut segments = Vec::new();
    let mut foliage = Vec::new();
    let height = rng.random_range(spec.body_plan.max_height[0]..spec.body_plan.max_height[1]);
    let trunk_radius = height * spec.trunk.thickness_ratio;
    let stem_count = spec.body_plan.stem_count.max(1);

    if stem_count <= 1 {
        generate_stem(
            spec,
            &mut rng,
            Vec3::ZERO,
            Vec3::Y,
            height,
            trunk_radius,
            &mut segments,
            &mut foliage,
        );
    } else {
        for i in 0..stem_count {
            let a = (i as f32 / stem_count as f32) * std::f32::consts::TAU;
            let spread = 0.3;
            let base = Vec3::new(a.cos() * spread, 0.0, a.sin() * spread);
            let dir = Vec3::new(a.cos() * 0.2, 1.0, a.sin() * 0.2).normalize();
            let h = height * rng.random_range(0.65..1.0);
            let r = trunk_radius * rng.random_range(0.4..0.7);
            generate_stem(spec, &mut rng, base, dir, h, r, &mut segments, &mut foliage);
        }
    }

    TreeData {
        segments,
        foliage,
        height,
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_stem(
    spec: &SpeciesConfig,
    rng: &mut StdRng,
    base: Vec3,
    direction: Vec3,
    height: f32,
    base_radius: f32,
    segments: &mut Vec<BranchSegment>,
    foliage: &mut Vec<FoliageBlob>,
) {
    let n_seg = 6;
    let top_radius = base_radius * (1.0 - spec.trunk.taper);
    let flare_radius = base_radius * (1.0 + spec.trunk.base_flare);

    let mut dir = direction;
    let mut pos = base;
    let mut trunk_pts = vec![pos];
    let mut trunk_dirs = vec![dir];

    for i in 0..n_seg {
        let t0 = i as f32 / n_seg as f32;
        let t1 = (i + 1) as f32 / n_seg as f32;
        let wobble = (1.0 - spec.trunk.straightness) * 0.06;
        dir = Vec3::new(
            dir.x + rng.random_range(-wobble..wobble),
            dir.y,
            dir.z + rng.random_range(-wobble..wobble),
        )
        .normalize();

        let seg_len = height / n_seg as f32;
        let next = pos + dir * seg_len;
        let r0 = if i == 0 {
            flare_radius
        } else {
            base_radius + (top_radius - base_radius) * t0
        };
        let r1 = base_radius + (top_radius - base_radius) * t1;

        segments.push(BranchSegment {
            start: pos,
            end: next,
            start_radius: r0,
            end_radius: r1,
            depth: 0,
        });
        pos = next;
        trunk_pts.push(pos);
        trunk_dirs.push(dir);
    }

    if spec.crown.shape == "fan_top" || spec.foliage.style == "palm_frond" {
        generate_fronds(spec, rng, pos, height, top_radius, segments, foliage);
        return;
    }

    let crown_start = height * spec.crown.crown_base;
    let crown_height = height - crown_start;
    if crown_height <= 0.0 {
        return;
    }

    let inter_node = height * 0.065;
    let num_nodes = (crown_height / inter_node).ceil().max(4.0) as u32;

    let mut arrangement_rot = rng.random::<f32>() * std::f32::consts::TAU;
    let arr_step = if spec.branching.arrangement.kind == "spiral" {
        (spec.branching.arrangement.angle.unwrap_or(137.5)).to_radians()
    } else if spec.branching.arrangement.kind == "opposite" {
        std::f32::consts::PI
    } else {
        0.0
    };

    for n in 0..num_nodes {
        let t_crown = (n as f32 + 0.5) / num_nodes as f32;
        let t_trunk = (crown_start + t_crown * crown_height) / height;

        let ti = t_trunk * n_seg as f32;
        let idx = (ti.floor() as usize).min(n_seg - 1);
        let frac = ti - idx as f32;
        let origin = trunk_pts[idx].lerp(trunk_pts[idx + 1], frac);
        let local_dir = trunk_dirs[idx].lerp(trunk_dirs[idx + 1], frac).normalize();

        let profile_scale = length_profile(&spec.branching.length_profile, t_crown);
        let max_len = crown_height * spec.crown.aspect_ratio * 0.4;
        let base_len = max_len * profile_scale * (1.0 - spec.branching.apical_dominance * 0.3);
        let thick_here = base_radius * (1.0 - t_trunk * spec.trunk.taper * 0.7);
        let branch_thick = thick_here * spec.branching.child_thickness_ratio;

        let count = rng.random_range(
            spec.branching.branches_per_node[0]..=spec.branching.branches_per_node[1],
        );

        if spec.branching.arrangement.kind == "whorled" {
            arrangement_rot = rng.random::<f32>() * std::f32::consts::TAU;
        }

        for _b in 0..count {
            let ang_base = rng.random_range(
                spec.branching.insertion_angle.base[0]..spec.branching.insertion_angle.base[1],
            );
            let ang_tip = rng.random_range(
                spec.branching.insertion_angle.tip[0]..spec.branching.insertion_angle.tip[1],
            );
            let insert_deg = ang_base + (ang_tip - ang_base) * t_trunk;
            let insert_rad = insert_deg.to_radians();
            let random_rot = spec.branching.randomness * rng.random_range(-0.3..0.3);
            let br_dir = branch_dir_3d(local_dir, insert_rad, arrangement_rot + random_rot);

            let len = base_len * rng.random_range(0.7..1.3);

            if spec.branching.arrangement.kind == "whorled" {
                arrangement_rot += std::f32::consts::TAU / count as f32;
            } else if arr_step > 0.0 {
                arrangement_rot += arr_step;
            } else {
                arrangement_rot = rng.random::<f32>() * std::f32::consts::TAU;
            }

            generate_branch(
                spec,
                rng,
                origin,
                br_dir,
                len,
                branch_thick,
                1,
                height,
                segments,
                foliage,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_branch(
    spec: &SpeciesConfig,
    rng: &mut StdRng,
    origin: Vec3,
    direction: Vec3,
    length: f32,
    thickness: f32,
    depth: u32,
    tree_height: f32,
    segments: &mut Vec<BranchSegment>,
    foliage: &mut Vec<FoliageBlob>,
) {
    if length < 0.08 || thickness < 0.005 {
        return;
    }

    let raw_end = origin + direction * length;
    let grav_drop = spec.branching.gravity_response * length * length * 0.04;
    let end = Vec3::new(raw_end.x, raw_end.y - grav_drop, raw_end.z);

    if !is_inside_crown(
        &spec.crown.shape,
        end,
        tree_height,
        spec.crown.crown_base,
        spec.crown.aspect_ratio,
    ) {
        if spec.foliage.style != "none" {
            let mid = (origin + end) * 0.5;
            let r = rng.random_range(spec.foliage.leaf_size[0]..spec.foliage.leaf_size[1])
                * tree_height
                * 0.06;
            foliage.push(FoliageBlob {
                center: mid,
                radius: r.max(0.2),
                hue_shift: rng.random_range(-15.0..15.0),
                light_shift: rng.random_range(-0.08..0.08),
            });
        }
        return;
    }

    let end_r = (thickness * (1.0 - spec.trunk.taper * 0.3)).max(0.005);
    segments.push(BranchSegment {
        start: origin,
        end,
        start_radius: thickness,
        end_radius: end_r,
        depth,
    });

    if depth >= spec.branching.max_depth {
        if spec.foliage.style != "none" {
            add_foliage(spec, rng, end, tree_height, foliage);
        }
        return;
    }

    let eff_dir = (end - origin).normalize();

    if spec.branching.apical_dominance > 0.2 {
        let cont_len = length
            * spec.branching.child_length_ratio
            * (0.5 + 0.5 * spec.branching.apical_dominance);
        let cont_thick = thickness * spec.branching.child_thickness_ratio;
        let cont_dir = (eff_dir
            + Vec3::new(
                rng.random_range(-0.05..0.05) * spec.branching.randomness,
                0.0,
                rng.random_range(-0.05..0.05) * spec.branching.randomness,
            ))
        .normalize();
        generate_branch(
            spec,
            rng,
            end,
            cont_dir,
            cont_len,
            cont_thick,
            depth + 1,
            tree_height,
            segments,
            foliage,
        );
    }

    let num_children =
        rng.random_range(spec.branching.branches_per_node[0]..=spec.branching.branches_per_node[1]);
    let mut child_rot = rng.random::<f32>() * std::f32::consts::TAU;
    for _i in 0..num_children {
        let spread_angle = rng.random_range(0.3..0.8);
        let random_rot = spec.branching.randomness * rng.random_range(-0.3..0.3);
        let child_dir = branch_dir_3d(eff_dir, spread_angle, child_rot + random_rot);
        let child_len = length * spec.branching.child_length_ratio * rng.random_range(0.6..1.1);
        let child_thick = thickness * spec.branching.child_thickness_ratio;
        child_rot += std::f32::consts::PI * 0.8 + rng.random_range(-0.2..0.2);
        generate_branch(
            spec,
            rng,
            end,
            child_dir,
            child_len,
            child_thick,
            depth + 1,
            tree_height,
            segments,
            foliage,
        );
    }
}

fn add_foliage(
    spec: &SpeciesConfig,
    rng: &mut StdRng,
    pos: Vec3,
    tree_height: f32,
    foliage: &mut Vec<FoliageBlob>,
) {
    let variance = spec.color.leaf_variance.unwrap_or(0.15);
    let size_base = tree_height * 0.045 * (1.0 + spec.crown.density * 0.5);
    let strategy = &spec.foliage.cluster_strategy;
    let blob_count = if strategy.kind == "dense_mass" {
        (4.0 * spec.crown.density).ceil() as u32
    } else if strategy.kind == "clusters" {
        strategy.count.unwrap_or(3)
    } else {
        1
    };
    let spread = if strategy.kind == "dense_mass" {
        size_base * 1.2
    } else {
        size_base * 0.6
    };

    for _i in 0..blob_count {
        foliage.push(FoliageBlob {
            center: Vec3::new(
                pos.x + rng.random_range(-spread..spread),
                pos.y + rng.random_range(-spread * 0.5..spread * 0.6),
                pos.z + rng.random_range(-spread..spread),
            ),
            radius: (size_base * rng.random_range(0.5..1.3)).max(0.15),
            hue_shift: rng.random_range(-1.0..1.0) * variance * 100.0,
            light_shift: rng.random_range(-1.0..1.0) * variance,
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn generate_fronds(
    spec: &SpeciesConfig,
    rng: &mut StdRng,
    apex: Vec3,
    tree_height: f32,
    top_radius: f32,
    segments: &mut Vec<BranchSegment>,
    foliage: &mut Vec<FoliageBlob>,
) {
    let frond_count = if spec.foliage.cluster_strategy.kind == "ring" {
        spec.foliage.cluster_strategy.count.unwrap_or(16)
    } else {
        14
    };
    let frond_length = tree_height * 0.3;
    let variance = spec.color.leaf_variance.unwrap_or(0.15);

    for i in 0..frond_count {
        let angle = (i as f32 / frond_count as f32) * std::f32::consts::TAU;
        let droop = spec.foliage.droop * frond_length * 0.4;
        let dx = angle.cos() * frond_length * 0.8;
        let dz = angle.sin() * frond_length * 0.8;
        let dy = frond_length * 0.3 - droop;
        let end = Vec3::new(apex.x + dx, apex.y + dy, apex.z + dz);

        segments.push(BranchSegment {
            start: apex,
            end,
            start_radius: top_radius * 0.25,
            end_radius: top_radius * 0.05,
            depth: 1,
        });

        for j in 0..5 {
            let ft = 0.25 + j as f32 * 0.15;
            foliage.push(FoliageBlob {
                center: Vec3::new(
                    apex.x + dx * ft + rng.random_range(-0.3..0.3),
                    apex.y + dy * ft,
                    apex.z + dz * ft + rng.random_range(-0.3..0.3),
                ),
                radius: tree_height * 0.03 * (1.2 - ft * 0.5),
                hue_shift: rng.random_range(-1.0..1.0) * variance * 80.0,
                light_shift: rng.random_range(-1.0..1.0) * variance * 0.8,
            });
        }
    }
}
