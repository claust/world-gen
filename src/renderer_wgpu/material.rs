use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Zeroable, Pod)]
pub struct FrameUniform {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub light_view_proj: [[f32; 4]; 4],
    pub camera_position: [f32; 4],
    /// x = elapsed seconds, y = hour-of-day, z = underwater submerge factor
    /// (0.0 above water → 1.0 fully submerged), w = unused.
    pub time: [f32; 4],
    /// Shadow controls: x = 1/shadow_map_size (texel), y = depth bias,
    /// z = shadow strength, w = enabled (1.0 = on, 0.0 = off).
    pub shadow_params: [f32; 4],
}

/// Shader depth bias applied when sampling the shadow map (in NDC depth units).
pub const SHADOW_DEPTH_BIAS: f32 = 0.0015;

impl FrameUniform {
    /// Build frame uniforms with shadows disabled (identity light matrix).
    /// Used by offscreen/thumbnail rendering that doesn't run a shadow pass.
    pub fn new(view_proj: Mat4, camera_position: Vec3, elapsed: f32, hour: f32) -> Self {
        Self::with_shadow(
            view_proj,
            camera_position,
            elapsed,
            hour,
            Mat4::IDENTITY,
            0.0,
            0.0,
        )
    }

    /// Build frame uniforms including the sun's light view-projection matrix.
    /// `shadow_enabled` is 1.0 during the day and 0.0 at night. `submerge` is the
    /// underwater factor (0.0 above water, ramping to 1.0 just below sea level);
    /// it is packed into `time.z` and read by the shared `scene_fog` helper.
    #[allow(clippy::too_many_arguments)]
    pub fn with_shadow(
        view_proj: Mat4,
        camera_position: Vec3,
        elapsed: f32,
        hour: f32,
        light_view_proj: Mat4,
        shadow_enabled: f32,
        submerge: f32,
    ) -> Self {
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            light_view_proj: light_view_proj.to_cols_array_2d(),
            camera_position: [camera_position.x, camera_position.y, camera_position.z, 0.0],
            time: [elapsed, hour, submerge, 0.0],
            shadow_params: [
                1.0 / super::pipeline::SHADOW_SIZE as f32,
                SHADOW_DEPTH_BIAS,
                1.0,
                shadow_enabled,
            ],
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
    pub sun_color: [f32; 4],
    pub sky_zenith: [f32; 4],
    pub sky_horizon: [f32; 4],
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
            sun_color: [1.0, 1.0, 0.95, 0.0],
            sky_zenith: [0.35, 0.55, 0.90, 0.0],
            sky_horizon: [0.55, 0.75, 0.95, 0.0],
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
        fog: [f32; 4],
        palette: &super::sky::SkyPalette,
    ) {
        let p = palette;
        let data = TerrainMaterialUniform {
            light_direction: [light_dir.x, light_dir.y, light_dir.z, 0.0],
            ambient: [ambient, ambient, ambient, 0.0],
            fog_color: [p.horizon[0], p.horizon[1], p.horizon[2], 1.0],
            fog_params: fog,
            sun_color: [p.sun_color[0], p.sun_color[1], p.sun_color[2], 0.0],
            sky_zenith: [p.zenith[0], p.zenith[1], p.zenith[2], 0.0],
            sky_horizon: [p.horizon[0], p.horizon[1], p.horizon[2], 0.0],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&[data]));
    }
}
