use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
pub struct FrameUniform {
    pub view_proj: [[f32; 4]; 4],
    pub camera_position: [f32; 4],
    pub time: [f32; 4],
}

impl FrameUniform {
    pub fn new(view_proj: Mat4, camera_position: Vec3, elapsed: f32, hour: f32) -> Self {
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            camera_position: [camera_position.x, camera_position.y, camera_position.z, 0.0],
            time: [elapsed, hour, 0.0, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
pub struct TerrainMaterialUniform {
    pub light_direction: [f32; 4],
    pub ambient: [f32; 4],
    pub fog_color: [f32; 4],
    pub fog_params: [f32; 4],
}

pub struct FrameBindGroup {
    pub layout: wgpu::BindGroupLayout,
    pub buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}

impl FrameBindGroup {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame-bind-group-layout"),
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

        let initial = FrameUniform::new(Mat4::IDENTITY, Vec3::ZERO, 0.0, 0.0);
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("frame-uniform-buffer"),
            contents: bytemuck::cast_slice(&[initial]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame-bind-group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        Self {
            layout,
            buffer,
            bind_group,
        }
    }

    pub fn update(&self, queue: &wgpu::Queue, uniform: &FrameUniform) {
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&[*uniform]));
    }
}

pub struct MaterialBindGroup {
    pub layout: wgpu::BindGroupLayout,
    pub buffer: wgpu::Buffer,
    pub bind_group: wgpu::BindGroup,
}

impl MaterialBindGroup {
    pub fn new_terrain(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain-material-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let initial = TerrainMaterialUniform {
            light_direction: [0.4, 1.0, 0.3, 0.0],
            ambient: [0.2, 0.2, 0.2, 0.0],
            fog_color: [0.45, 0.68, 0.96, 1.0],
            fog_params: [0.0, 1000.0, 0.0, 0.0],
        };

        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain-material-buffer"),
            contents: bytemuck::cast_slice(&[initial]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain-material-bind-group"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });

        Self {
            layout,
            buffer,
            bind_group,
        }
    }

    pub fn update_terrain(
        &self,
        queue: &wgpu::Queue,
        light_dir: Vec3,
        ambient: f32,
        fog_color: [f32; 3],
        fog_start: f32,
        fog_end: f32,
    ) {
        let data = TerrainMaterialUniform {
            light_direction: [light_dir.x, light_dir.y, light_dir.z, 0.0],
            ambient: [ambient, ambient, ambient, 0.0],
            fog_color: [fog_color[0], fog_color[1], fog_color[2], 1.0],
            fog_params: [fog_start, fog_end, 0.0, 0.0],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&[data]));
    }
}
