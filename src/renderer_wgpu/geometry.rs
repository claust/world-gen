use bytemuck::{Pod, Zeroable};
use glam::Vec3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Zeroable, Pod)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

pub fn append_box(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    center: Vec3,
    half_extents: Vec3,
    color: Vec3,
) {
    let min = center - half_extents;
    let max = center + half_extents;

    let p = [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(max.x, max.y, max.z),
        Vec3::new(min.x, max.y, max.z),
    ];

    append_quad(
        vertices,
        indices,
        [p[0], p[1], p[2], p[3]],
        Vec3::NEG_Z,
        color,
    );
    append_quad(vertices, indices, [p[5], p[4], p[7], p[6]], Vec3::Z, color);
    append_quad(
        vertices,
        indices,
        [p[4], p[0], p[3], p[7]],
        Vec3::NEG_X,
        color,
    );
    append_quad(vertices, indices, [p[1], p[5], p[6], p[2]], Vec3::X, color);
    append_quad(vertices, indices, [p[3], p[2], p[6], p[7]], Vec3::Y, color);
    append_quad(
        vertices,
        indices,
        [p[4], p[5], p[1], p[0]],
        Vec3::NEG_Y,
        color,
    );
}

pub fn append_octahedron(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    center: Vec3,
    radius: f32,
    color: Vec3,
) {
    let top = center + Vec3::Y * radius;
    let bottom = center - Vec3::Y * (radius * 0.9);
    let east = center + Vec3::X * radius;
    let west = center - Vec3::X * radius;
    let north = center + Vec3::Z * radius;
    let south = center - Vec3::Z * radius;

    append_triangle(vertices, indices, top, north, east, color);
    append_triangle(vertices, indices, top, east, south, color);
    append_triangle(vertices, indices, top, south, west, color);
    append_triangle(vertices, indices, top, west, north, color);
    append_triangle(vertices, indices, bottom, east, north, color);
    append_triangle(vertices, indices, bottom, south, east, color);
    append_triangle(vertices, indices, bottom, west, south, color);
    append_triangle(vertices, indices, bottom, north, west, color);
}

pub fn append_quad(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    corners: [Vec3; 4],
    normal: Vec3,
    color: Vec3,
) {
    let [a, b, c, d] = corners;
    let base = vertices.len() as u32;
    vertices.extend_from_slice(&[
        Vertex {
            position: a.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        Vertex {
            position: b.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        Vertex {
            position: c.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        Vertex {
            position: d.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
    ]);
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

pub fn append_triangle(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    a: Vec3,
    b: Vec3,
    c: Vec3,
    color: Vec3,
) {
    let normal = (b - a).cross(c - a).normalize_or_zero();
    let base = vertices.len() as u32;
    vertices.extend_from_slice(&[
        Vertex {
            position: a.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        Vertex {
            position: b.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        Vertex {
            position: c.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
    ]);
    indices.extend_from_slice(&[base, base + 1, base + 2]);
}
