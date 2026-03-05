use wgpu::util::DeviceExt;

const TILE_SIZE: u32 = 128;
const TILE_COUNT: u32 = 5;
const ATLAS_WIDTH: u32 = TILE_SIZE * TILE_COUNT;
const ATLAS_HEIGHT: u32 = TILE_SIZE;

pub struct TerrainTexture {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl TerrainTexture {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let pixels = generate_atlas_pixels();

        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("terrain-atlas"),
                size: wgpu::Extent3d {
                    width: ATLAS_WIDTH,
                    height: ATLAS_HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pixels,
        );

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("terrain-atlas-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain-texture-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("terrain-texture-bind-group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            bind_group_layout,
            bind_group,
        }
    }
}

/// Simple hash-based value noise for texture generation.
fn hash(x: i32, y: i32) -> f32 {
    let n = (x.wrapping_mul(374761393)).wrapping_add(y.wrapping_mul(668265263));
    let n = (n ^ (n >> 13)).wrapping_mul(1274126177);
    let n = n ^ (n >> 16);
    (n & 0x7fff) as f32 / 0x7fff as f32
}

fn value_noise(x: f32, y: f32) -> f32 {
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let fx = x - x.floor();
    let fy = y - y.floor();
    // Smoothstep
    let fx = fx * fx * (3.0 - 2.0 * fx);
    let fy = fy * fy * (3.0 - 2.0 * fy);

    let a = hash(ix, iy);
    let b = hash(ix + 1, iy);
    let c = hash(ix, iy + 1);
    let d = hash(ix + 1, iy + 1);

    let ab = a + (b - a) * fx;
    let cd = c + (d - c) * fx;
    ab + (cd - ab) * fy
}

fn fbm(x: f32, y: f32, octaves: u32) -> f32 {
    let mut val = 0.0;
    let mut amp = 0.5;
    let mut freq = 1.0;
    for _ in 0..octaves {
        val += value_noise(x * freq, y * freq) * amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    val
}

fn generate_atlas_pixels() -> Vec<u8> {
    let mut pixels = vec![0u8; (ATLAS_WIDTH * ATLAS_HEIGHT * 4) as usize];

    for tile in 0..TILE_COUNT {
        for py in 0..TILE_SIZE {
            for px in 0..TILE_SIZE {
                let nx = px as f32 / TILE_SIZE as f32;
                let ny = py as f32 / TILE_SIZE as f32;

                let (r, g, b) = match tile {
                    0 => generate_grass(nx, ny),
                    1 => generate_desert(nx, ny),
                    2 => generate_forest(nx, ny),
                    3 => generate_rock(nx, ny),
                    4 => generate_snow(nx, ny),
                    _ => unreachable!(),
                };

                let ax = tile * TILE_SIZE + px;
                let idx = ((py * ATLAS_WIDTH + ax) * 4) as usize;
                pixels[idx] = (r.clamp(0.0, 1.0) * 255.0) as u8;
                pixels[idx + 1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
                pixels[idx + 2] = (b.clamp(0.0, 1.0) * 255.0) as u8;
                pixels[idx + 3] = 255;
            }
        }
    }

    pixels
}

fn generate_grass(x: f32, y: f32) -> (f32, f32, f32) {
    let n1 = fbm(x * 8.0, y * 8.0, 4);
    let n2 = value_noise(x * 16.0, y * 16.0);
    let v = 0.4 + n1 * 0.3 + n2 * 0.1;
    (0.28 * v + 0.06, 0.50 * v + 0.12, 0.18 * v + 0.04)
}

fn generate_desert(x: f32, y: f32) -> (f32, f32, f32) {
    let n1 = fbm(x * 6.0, y * 6.0, 3);
    let n2 = value_noise(x * 20.0, y * 20.0);
    let v = 0.5 + n1 * 0.25 + n2 * 0.08;
    (0.72 * v + 0.10, 0.62 * v + 0.08, 0.36 * v + 0.06)
}

fn generate_forest(x: f32, y: f32) -> (f32, f32, f32) {
    let n1 = fbm(x * 8.0, y * 8.0, 4);
    let n2 = value_noise(x * 12.0, y * 12.0);
    let v = 0.35 + n1 * 0.3 + n2 * 0.1;
    (0.16 * v + 0.04, 0.40 * v + 0.10, 0.16 * v + 0.04)
}

fn generate_rock(x: f32, y: f32) -> (f32, f32, f32) {
    let n1 = fbm(x * 5.0, y * 5.0, 4);
    let n2 = value_noise(x * 15.0, y * 15.0);
    // Crack-like lines
    let crack = (((x * 12.0).sin() + (y * 8.0).cos()) * 3.0).abs().min(1.0);
    let v = 0.35 + n1 * 0.25 + n2 * 0.1 + crack * 0.05;
    (0.46 * v + 0.15, 0.46 * v + 0.15, 0.48 * v + 0.14)
}

fn generate_snow(x: f32, y: f32) -> (f32, f32, f32) {
    let n1 = fbm(x * 4.0, y * 4.0, 3);
    let n2 = value_noise(x * 10.0, y * 10.0);
    let v = 0.85 + n1 * 0.08 + n2 * 0.04;
    // Subtle blue tint
    (v - 0.02, v, v + 0.02)
}
