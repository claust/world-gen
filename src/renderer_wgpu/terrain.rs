use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{IVec2, Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::renderer_wgpu::pipeline::DepthTexture;
use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    ChunkData, ChunkTerrain, HouseInstance, TreeInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};

struct GpuChunk {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Zeroable, Pod)]
struct TerrainVertex {
    position: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
struct TerrainUniform {
    view_proj: [[f32; 4]; 4],
    light_direction: [f32; 4],
    ambient: [f32; 4],
}

pub struct TerrainRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    depth: DepthTexture,
    terrain_chunk_meshes: HashMap<IVec2, GpuChunk>,
    tree_chunk_meshes: HashMap<IVec2, GpuChunk>,
    house_chunk_meshes: HashMap<IVec2, GpuChunk>,
}

impl TerrainRenderer {
    pub fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Self {
        let initial_uniform = TerrainUniform {
            view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            light_direction: [0.4, 1.0, 0.3, 0.0],
            ambient: [0.2, 0.2, 0.2, 0.0],
        };

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain-uniform-buffer"),
            contents: bytemuck::cast_slice(&[initial_uniform]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain-bind-group-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain-bind-group"),
            layout: &bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/terrain.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain-pipeline-layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TerrainVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 24,
                    shader_location: 2,
                    format: wgpu::VertexFormat::Float32x3,
                },
            ],
        };

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[vertex_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: crate::renderer_wgpu::pipeline::DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });

        Self {
            pipeline,
            uniform_buffer,
            uniform_bind_group,
            depth: DepthTexture::new(device, config, "terrain-depth"),
            terrain_chunk_meshes: HashMap::new(),
            tree_chunk_meshes: HashMap::new(),
            house_chunk_meshes: HashMap::new(),
        }
    }

    pub fn resize(&mut self, device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) {
        self.depth = DepthTexture::new(device, config, "terrain-depth");
    }

    pub fn update_uniforms(
        &self,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        light_direction: Vec3,
        ambient: f32,
    ) {
        let data = TerrainUniform {
            view_proj: view_proj.to_cols_array_2d(),
            light_direction: [light_direction.x, light_direction.y, light_direction.z, 0.0],
            ambient: [ambient, ambient, ambient, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[data]));
    }

    pub fn sync_chunks(&mut self, device: &wgpu::Device, chunks: &HashMap<IVec2, ChunkData>) {
        self.terrain_chunk_meshes
            .retain(|coord, _| chunks.contains_key(coord));
        self.tree_chunk_meshes
            .retain(|coord, _| chunks.contains_key(coord));
        self.house_chunk_meshes
            .retain(|coord, _| chunks.contains_key(coord));

        for (coord, chunk) in chunks {
            if !self.terrain_chunk_meshes.contains_key(coord) {
                if let Some(cpu_mesh) = build_mesh(&chunk.terrain, &chunk.biome_map) {
                    let vertex_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("terrain-vertex-buffer"),
                            contents: bytemuck::cast_slice(cpu_mesh.vertices.as_slice()),
                            usage: wgpu::BufferUsages::VERTEX,
                        });

                    let index_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("terrain-index-buffer"),
                            contents: bytemuck::cast_slice(cpu_mesh.indices.as_slice()),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                    self.terrain_chunk_meshes.insert(
                        *coord,
                        GpuChunk {
                            vertex_buffer,
                            index_buffer,
                            index_count: cpu_mesh.indices.len() as u32,
                        },
                    );
                }
            }

            if !self.tree_chunk_meshes.contains_key(coord) {
                if let Some(cpu_mesh) = build_tree_mesh(&chunk.content.trees) {
                    let vertex_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("tree-vertex-buffer"),
                            contents: bytemuck::cast_slice(cpu_mesh.vertices.as_slice()),
                            usage: wgpu::BufferUsages::VERTEX,
                        });

                    let index_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("tree-index-buffer"),
                            contents: bytemuck::cast_slice(cpu_mesh.indices.as_slice()),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                    self.tree_chunk_meshes.insert(
                        *coord,
                        GpuChunk {
                            vertex_buffer,
                            index_buffer,
                            index_count: cpu_mesh.indices.len() as u32,
                        },
                    );
                }
            }

            if !self.house_chunk_meshes.contains_key(coord) {
                if let Some(cpu_mesh) = build_house_mesh(&chunk.content.houses) {
                    let vertex_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("house-vertex-buffer"),
                            contents: bytemuck::cast_slice(cpu_mesh.vertices.as_slice()),
                            usage: wgpu::BufferUsages::VERTEX,
                        });

                    let index_buffer =
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("house-index-buffer"),
                            contents: bytemuck::cast_slice(cpu_mesh.indices.as_slice()),
                            usage: wgpu::BufferUsages::INDEX,
                        });

                    self.house_chunk_meshes.insert(
                        *coord,
                        GpuChunk {
                            vertex_buffer,
                            index_buffer,
                            index_count: cpu_mesh.indices.len() as u32,
                        },
                    );
                }
            }
        }
    }

    pub fn render<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bind_group, &[]);

        for mesh in self.terrain_chunk_meshes.values() {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }

        for mesh in self.tree_chunk_meshes.values() {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }

        for mesh in self.house_chunk_meshes.values() {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }

    pub fn depth_view(&self) -> &wgpu::TextureView {
        &self.depth.view
    }
}

struct CpuChunkMesh {
    vertices: Vec<TerrainVertex>,
    indices: Vec<u32>,
}

fn build_mesh(chunk: &ChunkTerrain, biome_map: &BiomeMap) -> Option<CpuChunkMesh> {
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

    let vertices: Vec<TerrainVertex> = (0..total)
        .map(|idx| {
            let x = idx % side;
            let z = idx / side;
            let world_x = origin_x + x as f32 * cell_size;
            let world_z = origin_z + z as f32 * cell_size;
            let h = chunk.heights[idx];
            let biome = biome_map.values[idx];
            let color = biome_ground_color(biome, h);
            let n = normals[idx];
            TerrainVertex {
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

fn build_tree_mesh(trees: &[TreeInstance]) -> Option<CpuChunkMesh> {
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

fn build_house_mesh(houses: &[HouseInstance]) -> Option<CpuChunkMesh> {
    if houses.is_empty() {
        return None;
    }

    let mut vertices = Vec::with_capacity(houses.len() * 48);
    let mut indices = Vec::with_capacity(houses.len() * 72);

    let wall_color = Vec3::new(0.72, 0.63, 0.46);
    let roof_color = Vec3::new(0.55, 0.22, 0.15);

    // House dimensions (local space, before rotation)
    let half_w = 2.5; // half-width along X (long side)
    let half_d = 2.0; // half-depth along Z
    let wall_h = 3.0; // wall height
    let roof_h = 2.0; // roof peak above walls

    for house in houses {
        let cos_r = house.rotation.cos();
        let sin_r = house.rotation.sin();

        // Rotate a local (x, z) offset by house.rotation around Y
        let rot = |lx: f32, lz: f32| -> Vec3 {
            Vec3::new(lx * cos_r - lz * sin_r, 0.0, lx * sin_r + lz * cos_r)
        };

        let base = house.position;

        // 4 base corners and 4 top-of-wall corners
        let bl = base + rot(-half_w, -half_d);
        let br = base + rot(half_w, -half_d);
        let fr = base + rot(half_w, half_d);
        let fl = base + rot(-half_w, half_d);

        let tbl = bl + Vec3::Y * wall_h;
        let tbr = br + Vec3::Y * wall_h;
        let tfr = fr + Vec3::Y * wall_h;
        let tfl = fl + Vec3::Y * wall_h;

        // Roof ridge runs along the X axis (long side)
        let ridge_l = base + rot(-half_w, 0.0) + Vec3::Y * (wall_h + roof_h);
        let ridge_r = base + rot(half_w, 0.0) + Vec3::Y * (wall_h + roof_h);

        // --- Walls (4 sides) ---
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

        // --- Roof slopes (2 quads) ---
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

        // --- Gable ends (2 triangles) ---
        append_triangle(&mut vertices, &mut indices, tbl, tfl, ridge_l, roof_color);
        append_triangle(&mut vertices, &mut indices, tfr, tbr, ridge_r, roof_color);
    }

    Some(CpuChunkMesh { vertices, indices })
}

fn append_box(
    vertices: &mut Vec<TerrainVertex>,
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

fn append_octahedron(
    vertices: &mut Vec<TerrainVertex>,
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

fn append_quad(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    corners: [Vec3; 4],
    normal: Vec3,
    color: Vec3,
) {
    let [a, b, c, d] = corners;
    let base = vertices.len() as u32;
    vertices.extend_from_slice(&[
        TerrainVertex {
            position: a.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        TerrainVertex {
            position: b.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        TerrainVertex {
            position: c.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        TerrainVertex {
            position: d.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
    ]);
    indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

fn append_triangle(
    vertices: &mut Vec<TerrainVertex>,
    indices: &mut Vec<u32>,
    a: Vec3,
    b: Vec3,
    c: Vec3,
    color: Vec3,
) {
    let normal = (b - a).cross(c - a).normalize_or_zero();
    let base = vertices.len() as u32;
    vertices.extend_from_slice(&[
        TerrainVertex {
            position: a.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        TerrainVertex {
            position: b.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
        TerrainVertex {
            position: c.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        },
    ]);
    indices.extend_from_slice(&[base, base + 1, base + 2]);
}
