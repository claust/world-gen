// Tiling water normal map for the river flow-map pass (docs/RIVERS.md, Option D).
// A single modest, seamlessly-tiling normal texture (procedurally generated, in
// the same spirit as the terrain atlas) that `river.wgsl` advects along the
// per-vertex flow vector using the two-phase cross-fade technique. Mipmaps are
// generated on the CPU so the high-frequency ripples filter cleanly into the
// distance instead of aliasing into grazing-angle banding.

/// Side length of the base mip. Power-of-two so the full mip chain is clean.
const TILE_SIZE: u32 = 256;

pub struct RiverNormalTexture {
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl RiverNormalTexture {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mips = generate_normal_mips(TILE_SIZE);
        let mip_level_count = mips.len() as u32;

        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("river-normal-map"),
            size: wgpu::Extent3d {
                width: TILE_SIZE,
                height: TILE_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Linear (non-sRGB): these are surface normals, not colour.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        for (level, pixels) in mips.iter().enumerate() {
            let size = TILE_SIZE >> level;
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
            );
        }

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("river-normal-sampler"),
            // Tiles seamlessly across the world, so repeat.
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("river-normal-layout"),
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
            label: Some("river-normal-bind-group"),
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

/// 32-bit integer hash → `[0,1)`, on a toroidal lattice (caller wraps coords).
fn hash(x: i32, y: i32) -> f32 {
    let n = (x.wrapping_mul(374761393)).wrapping_add(y.wrapping_mul(668265263));
    let n = (n ^ (n >> 13)).wrapping_mul(1274126177);
    let n = n ^ (n >> 16);
    (n & 0x7fff) as f32 / 0x7fff as f32
}

/// Value noise that tiles seamlessly: lattice coordinates wrap modulo `cells`,
/// so `cells` whole periods fit exactly across the unit tile. `x`/`y` in `[0,1)`.
fn tiling_value_noise(x: f32, y: f32, cells: i32) -> f32 {
    let fx = x * cells as f32;
    let fy = y * cells as f32;
    let ix = fx.floor() as i32;
    let iy = fy.floor() as i32;
    let tx = fx - fx.floor();
    let ty = fy - fy.floor();
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);

    let wrap = |v: i32| v.rem_euclid(cells);
    let x0 = wrap(ix);
    let x1 = wrap(ix + 1);
    let y0 = wrap(iy);
    let y1 = wrap(iy + 1);

    let a = hash(x0, y0);
    let b = hash(x1, y0);
    let c = hash(x0, y1);
    let d = hash(x1, y1);
    let ab = a + (b - a) * sx;
    let cd = c + (d - c) * sx;
    ab + (cd - ab) * sy
}

/// Seamless fractal water height field at `(x, y)` in `[0,1)`. A few octaves of
/// tiling value noise; the gradient of this field becomes the surface normal.
fn tiling_fbm(x: f32, y: f32) -> f32 {
    let mut val = 0.0;
    let mut amp = 0.5;
    let mut cells = 4;
    for _ in 0..4 {
        val += tiling_value_noise(x, y, cells) * amp;
        amp *= 0.5;
        cells *= 2;
    }
    val
}

/// Build the base normal map plus its full mip chain (down to 1×1) by box
/// filtering. Each mip is RGBA8: RG = tangent-space normal XY (0.5-centred),
/// B = Z (up), A unused (255).
fn generate_normal_mips(base: u32) -> Vec<Vec<u8>> {
    // Base level: derive normals from the height field via central differences
    // on the torus. `STRENGTH` scales the ripple slope; higher = bumpier water.
    const STRENGTH: f32 = 2.2;
    let n = base as usize;
    let eps = 1.0 / base as f32;
    let mut base_px = vec![0u8; n * n * 4];
    for py in 0..n {
        for px in 0..n {
            let x = px as f32 / base as f32;
            let y = py as f32 / base as f32;
            let hl = tiling_fbm((x - eps).rem_euclid(1.0), y);
            let hr = tiling_fbm((x + eps).rem_euclid(1.0), y);
            let hd = tiling_fbm(x, (y - eps).rem_euclid(1.0));
            let hu = tiling_fbm(x, (y + eps).rem_euclid(1.0));
            let dx = (hr - hl) * STRENGTH;
            let dy = (hu - hd) * STRENGTH;
            // Normal = normalize(-dx, -dy, 1) in tangent space (Z up).
            let inv = 1.0 / (dx * dx + dy * dy + 1.0).sqrt();
            let nx = -dx * inv;
            let ny = -dy * inv;
            let nz = inv;
            let idx = (py * n + px) * 4;
            base_px[idx] = ((nx * 0.5 + 0.5) * 255.0).round() as u8;
            base_px[idx + 1] = ((ny * 0.5 + 0.5) * 255.0).round() as u8;
            base_px[idx + 2] = ((nz * 0.5 + 0.5) * 255.0).round() as u8;
            base_px[idx + 3] = 255;
        }
    }

    let mut mips = vec![base_px];
    let mut size = base;
    while size > 1 {
        let prev = mips.last().unwrap();
        let prev_n = size as usize;
        let next = size / 2;
        let next_n = next as usize;
        let mut out = vec![0u8; next_n * next_n * 4];
        for py in 0..next_n {
            for px in 0..next_n {
                for c in 0..4 {
                    let s00 = prev[((py * 2) * prev_n + px * 2) * 4 + c] as u32;
                    let s10 = prev[((py * 2) * prev_n + px * 2 + 1) * 4 + c] as u32;
                    let s01 = prev[((py * 2 + 1) * prev_n + px * 2) * 4 + c] as u32;
                    let s11 = prev[((py * 2 + 1) * prev_n + px * 2 + 1) * 4 + c] as u32;
                    out[(py * next_n + px) * 4 + c] = ((s00 + s10 + s01 + s11) / 4) as u8;
                }
            }
        }
        mips.push(out);
        size = next;
    }
    mips
}
