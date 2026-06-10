//! Near-field grass: instanced wind-swaying tufts in a camera-window tile
//! cache.
//!
//! Grass is a render-layer effect, not chunk content (see GRASS_PLAN.md): a
//! full 256 m chunk at 0.5 m tuft spacing would be ~260k instances, so tufts
//! are generated lazily in 32 m tiles around the camera and dropped with
//! hysteresis as it moves. Placement is the pure `world_core::content::grass`
//! module; this pass owns the tile cache, the shared tuft prototype mesh, the
//! pipeline, and distance-based thinning (a prefix draw over fade-key-sorted
//! instance buffers, matched by a shader-side fade so nothing pops).

use std::collections::HashMap;

use glam::{IVec2, Vec3};

use super::frustum::Frustum;
use super::geometry::Vertex;
use super::instancing::{
    upload_instances, upload_prototype, GpuInstanceChunk, InstanceData, PrototypeMesh,
};
use super::pipeline::create_billboard_pipeline;
use super::terrain_texture::ground_albedo_linear;
use crate::world_core::biome::Biome;
use crate::world_core::chunk::ChunkData;
use crate::world_core::config::BiomeConfig;
use crate::world_core::content::grass::{
    generate_grass_tile, GrassTuft, GRASS_TILES_PER_CHUNK, GRASS_TILE_SIZE,
};
use crate::world_core::content::sampling::{hash4, hash_to_unit_float};

/// Radius of full-density grass around the camera.
const GRASS_FULL_DISTANCE: f32 = 35.0;
/// Grass horizon: keep fraction (and tuft height) reach zero here.
const GRASS_FAR_DISTANCE: f32 = 90.0;
/// Beyond this tile distance the cheap 2-blade LOD mesh replaces the full
/// 5-blade tuft. Distant blades are subpixel anyway; what hurts the tile-based
/// GPU is raw micro-triangle count, so the far field gets 60% fewer triangles
/// at similar coverage (the LOD blades are wider).
const GRASS_LOD_DISTANCE: f32 = 40.0;
/// Tiles are generated when their nearest point comes within this radius...
const TILE_KEEP_RADIUS: f32 = GRASS_FAR_DISTANCE + 16.0;
/// ...and dropped beyond this one (hysteresis so border tiles don't churn).
const TILE_DROP_RADIUS: f32 = GRASS_FAR_DISTANCE + 48.0;
/// Per-frame tile generation budget — spreads initial fill / fast flight over
/// a few frames so chunk streaming never hitches.
const TILES_PER_FRAME: usize = 6;
/// The CPU draws this margin above the shader's keep fraction so the tufts
/// the shader is currently fading always exist in the draw. Must match
/// KEEP_MARGIN in grass.wgsl.
const KEEP_MARGIN: f32 = 1.12;
/// Blades per tuft in the near-field prototype mesh.
const BLADES_PER_TUFT: u32 = 5;
/// Blades per tuft in the far LOD mesh.
const BLADES_PER_TUFT_LOD: u32 = 2;
/// Nominal blade height in metres at instance scale 1.0.
const BLADE_HEIGHT: f32 = 0.50;

struct GrassTile {
    gpu: Option<GpuInstanceChunk>,
    /// Terrain height bounds of the owning chunk, for the frustum AABB.
    min_y: f32,
    max_y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GrassStats {
    pub tiles: usize,
    pub buffered_tufts: usize,
}

pub struct GrassPass {
    pipeline: wgpu::RenderPipeline,
    mesh: PrototypeMesh,
    mesh_lod: PrototypeMesh,
    tiles: HashMap<IVec2, GrassTile>,
    seed: u32,
    biome_config: BiomeConfig,
    sea_level: f32,
}

impl GrassPass {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        frame_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        shadow_layout: &wgpu::BindGroupLayout,
        seed: u32,
        biome_config: BiomeConfig,
        sea_level: f32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("grass-shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shaders/lighting.wgsl"),
                    include_str!("shaders/grass.wgsl")
                )
                .into(),
            ),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grass-pipeline-layout"),
            bind_group_layouts: &[frame_layout, material_layout, shadow_layout],
            push_constant_ranges: &[],
        });

        // Same slot layout as InstancedPass so InstanceData/Vertex helpers are
        // shared verbatim.
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
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
        let instance_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceData>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 3,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 12,
                    shader_location: 4,
                    format: wgpu::VertexFormat::Float32,
                },
                wgpu::VertexAttribute {
                    offset: 16,
                    shader_location: 5,
                    format: wgpu::VertexFormat::Float32x3,
                },
                wgpu::VertexAttribute {
                    offset: 32,
                    shader_location: 6,
                    format: wgpu::VertexFormat::Float32x4,
                },
                wgpu::VertexAttribute {
                    offset: 28,
                    shader_location: 7,
                    format: wgpu::VertexFormat::Float32,
                },
            ],
        };

        // Cull-off opaque pipeline: blades are two-sided, depth-tested and
        // depth-writing; no alpha test needed (real geometry, no cards).
        let pipeline = create_billboard_pipeline(
            device,
            render_format,
            &pipeline_layout,
            &shader,
            &[vertex_layout, instance_layout],
            "grass-pipeline",
        );

        let (verts, indices) = grass_tuft_mesh(BLADES_PER_TUFT, 1.0);
        let mesh = upload_prototype(device, &verts, &indices, "grass-tuft");
        let (lod_verts, lod_indices) = grass_tuft_mesh(BLADES_PER_TUFT_LOD, 1.6);
        let mesh_lod = upload_prototype(device, &lod_verts, &lod_indices, "grass-tuft-lod");

        Self {
            pipeline,
            mesh,
            mesh_lod,
            tiles: HashMap::new(),
            seed,
            biome_config,
            sea_level,
        }
    }

    /// Maintain the tile cache around the camera: drop tiles that strayed out
    /// of range or lost their chunk, generate the closest missing tiles within
    /// a per-frame budget.
    pub fn sync(
        &mut self,
        device: &wgpu::Device,
        world_chunks: &HashMap<IVec2, ChunkData>,
        camera_pos: Vec3,
    ) {
        let cam = camera_pos;
        self.tiles.retain(|tile, _| {
            world_chunks.contains_key(&chunk_of_tile(*tile))
                && tile_distance(*tile, cam) < TILE_DROP_RADIUS
        });

        // Candidate tiles: the square of tile coords covering the keep radius.
        let lo_x = ((cam.x - TILE_KEEP_RADIUS) / GRASS_TILE_SIZE).floor() as i32;
        let hi_x = ((cam.x + TILE_KEEP_RADIUS) / GRASS_TILE_SIZE).floor() as i32;
        let lo_z = ((cam.z - TILE_KEEP_RADIUS) / GRASS_TILE_SIZE).floor() as i32;
        let hi_z = ((cam.z + TILE_KEEP_RADIUS) / GRASS_TILE_SIZE).floor() as i32;

        let mut missing: Vec<(f32, IVec2)> = Vec::new();
        for tz in lo_z..=hi_z {
            for tx in lo_x..=hi_x {
                let tile = IVec2::new(tx, tz);
                if self.tiles.contains_key(&tile) {
                    continue;
                }
                let dist = tile_distance(tile, cam);
                if dist >= TILE_KEEP_RADIUS {
                    continue;
                }
                if !world_chunks.contains_key(&chunk_of_tile(tile)) {
                    continue;
                }
                missing.push((dist, tile));
            }
        }
        missing.sort_by(|a, b| a.0.total_cmp(&b.0));

        for (_, tile) in missing.into_iter().take(TILES_PER_FRAME) {
            let chunk_coord = chunk_of_tile(tile);
            let Some(chunk) = world_chunks.get(&chunk_coord) else {
                continue;
            };
            let tile_local = tile - chunk_coord * GRASS_TILES_PER_CHUNK;
            let tufts = generate_grass_tile(
                self.seed,
                &self.biome_config,
                self.sea_level,
                chunk_coord,
                tile_local,
                &chunk.terrain,
            );
            let instances: Vec<InstanceData> = tufts.iter().map(tuft_instance).collect();
            self.tiles.insert(
                tile,
                GrassTile {
                    gpu: upload_instances(device, &instances, "grass"),
                    min_y: chunk.terrain.min_height - 1.0,
                    max_y: chunk.terrain.max_height + BLADE_HEIGHT + 1.0,
                },
            );
        }
    }

    /// Draw all visible tiles. Per tile, the instance count is scaled by the
    /// distance keep-fraction (the buffer is sorted by fade key, so a prefix
    /// is a uniform subset); the shader fades the marginal tufts so the count
    /// changing frame-to-frame never pops. Grass casts no shadows, so there is
    /// no `render_depth` counterpart.
    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frustum: &Frustum,
        camera_position: Vec3,
        shadow_bind_group: &'a wgpu::BindGroup,
    ) {
        if self.tiles.is_empty() {
            return;
        }

        // Partition visible tiles so each mesh is bound once: full tufts near
        // the camera, the cheap wide 2-blade LOD beyond GRASS_LOD_DISTANCE.
        let mut near: Vec<(&GpuInstanceChunk, u32)> = Vec::new();
        let mut far: Vec<(&GpuInstanceChunk, u32)> = Vec::new();
        for (tile, entry) in &self.tiles {
            let Some(gpu) = &entry.gpu else { continue };

            let min = Vec3::new(
                tile.x as f32 * GRASS_TILE_SIZE,
                entry.min_y,
                tile.y as f32 * GRASS_TILE_SIZE,
            );
            let max = Vec3::new(
                min.x + GRASS_TILE_SIZE,
                entry.max_y,
                min.z + GRASS_TILE_SIZE,
            );
            if !frustum.is_aabb_visible(min, max) {
                continue;
            }

            let dist = tile_distance(*tile, camera_position);
            let count = ((gpu.instance_count as f32) * keep_fraction(dist)).ceil() as u32;
            if count == 0 {
                continue;
            }
            let count = count.min(gpu.instance_count);
            if dist < GRASS_LOD_DISTANCE {
                near.push((gpu, count));
            } else {
                far.push((gpu, count));
            }
        }
        if near.is_empty() && far.is_empty() {
            return;
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(2, shadow_bind_group, &[]);
        for (mesh, tiles) in [(&self.mesh, &near), (&self.mesh_lod, &far)] {
            if tiles.is_empty() {
                continue;
            }
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (gpu, count) in tiles {
                pass.set_vertex_buffer(1, gpu.instance_buffer.slice(..));
                pass.draw_indexed(0..mesh.index_count, 0, 0..*count);
            }
        }
    }

    pub fn stats(&self) -> GrassStats {
        GrassStats {
            tiles: self.tiles.len(),
            buffered_tufts: self
                .tiles
                .values()
                .filter_map(|t| t.gpu.as_ref())
                .map(|g| g.instance_count as usize)
                .sum(),
        }
    }
}

/// Fraction of a tile's (fade-key-sorted) instances to draw at a given
/// camera distance, including the KEEP_MARGIN headroom for the shader-side
/// fade. The cubic falloff thins aggressively right past the full-density
/// radius — at distance the blades are subpixel and their micro-triangle
/// count, not their look, is what costs. Must mirror vs_main in grass.wgsl.
fn keep_fraction(dist: f32) -> f32 {
    let t =
        ((GRASS_FAR_DISTANCE - dist) / (GRASS_FAR_DISTANCE - GRASS_FULL_DISTANCE)).clamp(0.0, 1.0);
    (t * t * t * KEEP_MARGIN).min(1.0)
}

/// Chunk coordinate owning a world tile coordinate.
fn chunk_of_tile(tile: IVec2) -> IVec2 {
    IVec2::new(
        tile.x.div_euclid(GRASS_TILES_PER_CHUNK),
        tile.y.div_euclid(GRASS_TILES_PER_CHUNK),
    )
}

/// Horizontal distance from the camera to the nearest point of a tile's
/// footprint (0 inside the tile). Using the nearest point keeps the CPU keep
/// fraction an upper bound of every per-instance fade in the shader, so the
/// prefix draw always contains the tufts the shader still shows.
fn tile_distance(tile: IVec2, camera: Vec3) -> f32 {
    let min_x = tile.x as f32 * GRASS_TILE_SIZE;
    let min_z = tile.y as f32 * GRASS_TILE_SIZE;
    let dx = camera.x - camera.x.clamp(min_x, min_x + GRASS_TILE_SIZE);
    let dz = camera.z - camera.z.clamp(min_z, min_z + GRASS_TILE_SIZE);
    (dx * dx + dz * dz).sqrt()
}

fn tuft_instance(t: &GrassTuft) -> InstanceData {
    let tint = ground_albedo_linear(biome_tile_index(t.biome), t.position.x, t.position.z);
    // Slightly darker than the ground sample so blade roots read as sitting
    // *in* the grass sward rather than glowing on top of it.
    InstanceData {
        position: [t.position.x, t.position.y, t.position.z],
        rotation_y: t.yaw,
        scale: [t.width_scale, t.height_scale, t.width_scale],
        tilt: 0.0,
        color: [tint[0] * 0.97, tint[1] * 0.97, tint[2] * 0.97, t.fade_key],
    }
}

/// Atlas tile order from terrain_gen.wgsl: 0=grass, 1=desert, 2=forest,
/// 3=rock, 4=snow.
fn biome_tile_index(biome: Biome) -> u32 {
    match biome {
        Biome::Grassland => 0,
        Biome::Desert => 1,
        Biome::Forest => 2,
        Biome::Rock => 3,
        Biome::Snow => 4,
    }
}

/// Build a tuft prototype: `blades` tapered two-segment blades fanned around
/// the tuft centre with hashed lean, height, and width (scaled by
/// `width_mul`; the far LOD uses fewer, wider blades to keep coverage while
/// shedding triangles). `Vertex.color` is repurposed as packed blade data for
/// the shader: r = bend weight (0 root -> 1 tip), g = per-blade phase,
/// b = base AO.
fn grass_tuft_mesh(blades: u32, width_mul: f32) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts: Vec<Vertex> = Vec::with_capacity(blades as usize * 5);
    let mut indices: Vec<u32> = Vec::with_capacity(blades as usize * 9);

    for i in 0..blades {
        let h = |salt: u32| hash_to_unit_float(hash4(0xB1AD_E000, i, salt, 0));

        let yaw = (i as f32 + h(1)) / blades as f32 * std::f32::consts::TAU;
        let dir = Vec3::new(yaw.sin(), 0.0, yaw.cos());
        let perp = Vec3::new(yaw.cos(), 0.0, -yaw.sin());

        let height = BLADE_HEIGHT * (0.65 + 0.35 * h(3));
        // Horizontal tip offset as a fraction of blade height — blades lean
        // outward so the tuft fans open instead of standing as a flat stack.
        let lean = 0.10 + 0.30 * h(2);
        let root = dir * (0.035 * h(4));
        let w0 = 0.042 * width_mul * (0.8 + 0.4 * h(5));
        let w1 = w0 * 0.55;

        let mid = root + dir * (lean * 0.35 * height) + Vec3::Y * (height * 0.55);
        let tip = root + dir * (lean * height) + Vec3::Y * (height * 0.96);
        let normal = (dir + Vec3::Y * 0.7).normalize();
        let phase = h(6);

        let base = verts.len() as u32;
        let v = |pos: Vec3, bend: f32, ao: f32| Vertex {
            position: [pos.x, pos.y, pos.z],
            normal: [normal.x, normal.y, normal.z],
            color: [bend, phase, ao],
        };
        verts.push(v(root - perp * w0, 0.0, 0.72));
        verts.push(v(root + perp * w0, 0.0, 0.72));
        verts.push(v(mid - perp * w1, 0.45, 0.92));
        verts.push(v(mid + perp * w1, 0.45, 0.92));
        verts.push(v(tip, 1.0, 1.0));

        // Winding is irrelevant: the pipeline is double-sided (cull off).
        indices.extend_from_slice(&[
            base,
            base + 2,
            base + 1,
            base + 1,
            base + 2,
            base + 3,
            base + 2,
            base + 4,
            base + 3,
        ]);
    }

    (verts, indices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tuft_mesh_has_expected_size_and_bend_range() {
        let (verts, indices) = grass_tuft_mesh(BLADES_PER_TUFT, 1.0);
        assert_eq!(verts.len(), BLADES_PER_TUFT as usize * 5);
        assert_eq!(indices.len(), BLADES_PER_TUFT as usize * 9);
        for v in &verts {
            assert!((0.0..=1.0).contains(&v.color[0]), "bend weight in range");
            assert!(v.position[1] >= 0.0 && v.position[1] <= BLADE_HEIGHT);
        }
        assert!(indices.iter().all(|&i| (i as usize) < verts.len()));
    }

    #[test]
    fn tile_to_chunk_mapping_handles_negative_coords() {
        assert_eq!(chunk_of_tile(IVec2::new(0, 0)), IVec2::new(0, 0));
        assert_eq!(chunk_of_tile(IVec2::new(7, 8)), IVec2::new(0, 1));
        assert_eq!(chunk_of_tile(IVec2::new(-1, -8)), IVec2::new(-1, -1));
        assert_eq!(chunk_of_tile(IVec2::new(-9, 15)), IVec2::new(-2, 1));
    }
}
