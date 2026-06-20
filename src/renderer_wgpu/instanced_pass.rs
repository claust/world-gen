use std::collections::HashMap;

use glam::{IVec2, Vec3};

use super::frustum::Frustum;
use super::geometry::Vertex;
use super::instancing::{
    build_house_instances, build_plant_instances, shrub_billboard_mesh, sign_post_mesh,
    upload_instances, upload_prototype, GpuInstanceChunk, InstanceData, ModelRegistry,
    PlantInstanceBuckets, PrototypeMesh,
};
#[cfg(not(target_arch = "wasm32"))]
use super::model_loader;
use super::pipeline::{create_billboard_pipeline, create_render_pipeline, create_shadow_pipeline};
use crate::world_core::chunk::{ChunkData, CHUNK_SIZE_METERS};
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::plant_gen;

/// Squared distance (in world units) beyond which chunks use LOD meshes.
const LOD_THRESHOLD_SQ: f32 = 512.0 * 512.0;

pub struct InstancedPass {
    pipeline: wgpu::RenderPipeline,
    /// Alpha-tested, double-sided pipeline for shrub billboards.
    billboard_pipeline: wgpu::RenderPipeline,
    /// Depth-only pipeline for rendering instances into the shadow map.
    shadow_pipeline: wgpu::RenderPipeline,
    models: ModelRegistry,
    /// species_names[i] = model key in ModelRegistry for species i
    species_names: Vec<String>,
    /// species_lod_names[i] = LOD model key for species i
    species_lod_names: Vec<String>,
    /// species_dead_names[i] = dead-snag model key for species i
    species_dead_names: Vec<String>,
    /// species_is_shrub[i] = true when species i renders as a billboard
    species_is_shrub: Vec<bool>,
    /// Mature plants; drawn full-res nearby and as LOD at distance.
    plant_instances: Vec<HashMap<IVec2, ChunkPlantBuffers>>,
    /// Seedlings and young plants; always drawn with LOD meshes.
    plant_lod_instances: Vec<HashMap<IVec2, ChunkPlantBuffers>>,
    /// Dead snags; drawn with the bark-only snag mesh at every distance.
    plant_dead_instances: Vec<HashMap<IVec2, ChunkPlantBuffers>>,
    house_instances: HashMap<IVec2, GpuInstanceChunk>,
    /// One debug road-post sign per chunk, placed at the chunk center.
    sign_instances: HashMap<IVec2, GpuInstanceChunk>,
}

struct ChunkPlantBuffers {
    revision: u64,
    gpu: Option<GpuInstanceChunk>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InstancedStats {
    pub buffered_mature_plants: usize,
    pub buffered_lod_plants: usize,
    pub buffered_dead_plants: usize,
    pub buffered_house_instances: usize,
}

impl InstancedPass {
    pub fn new(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        frame_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        shadow_layout: &wgpu::BindGroupLayout,
        registry: &PlantRegistry,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced-shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shaders/lighting.wgsl"),
                    include_str!("shaders/instanced.wgsl")
                )
                .into(),
            ),
        });

        // Lit pipelines sample the shadow map at group 2.
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("instanced-pipeline-layout"),
            bind_group_layouts: &[frame_layout, material_layout, shadow_layout],
            push_constant_ranges: &[],
        });

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

        let pipeline = create_render_pipeline(
            device,
            render_format,
            &pipeline_layout,
            &shader,
            &[vertex_layout.clone(), instance_layout.clone()],
            "instanced-pipeline",
        );

        let billboard_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shrub-billboard-shader"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(
                    include_str!("shaders/lighting.wgsl"),
                    include_str!("shaders/shrub_billboard.wgsl")
                )
                .into(),
            ),
        });
        let billboard_pipeline = create_billboard_pipeline(
            device,
            render_format,
            &pipeline_layout,
            &billboard_shader,
            &[vertex_layout.clone(), instance_layout.clone()],
            "shrub-billboard-pipeline",
        );

        // Depth-only shadow pipeline (group 0 / frame only). Reuses the same
        // vertex + instance layouts but only reads position and the per-instance
        // transform.
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced-shadow-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/shadows.wgsl").into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("instanced-shadow-pipeline-layout"),
                bind_group_layouts: &[frame_layout],
                push_constant_ranges: &[],
            });
        let shadow_pipeline = create_shadow_pipeline(
            device,
            &shadow_pipeline_layout,
            &shadow_shader,
            "vs_instanced",
            &[vertex_layout, instance_layout],
            "instanced-shadow-pipeline",
        );

        let mut models = ModelRegistry::new(device);
        let (species_names, species_lod_names, species_dead_names, species_is_shrub) =
            Self::build_species_meshes(device, &mut models, registry);

        // Debug tile-marker road-post sign (one shared prototype mesh).
        let (sign_verts, sign_indices) = sign_post_mesh();
        models.models.insert(
            "sign-post".to_string(),
            upload_prototype(device, &sign_verts, &sign_indices, "sign-post"),
        );

        let plant_instances = (0..registry.species.len())
            .map(|_| HashMap::new())
            .collect();

        Self {
            pipeline,
            billboard_pipeline,
            shadow_pipeline,
            models,
            species_names,
            species_lod_names,
            species_dead_names,
            species_is_shrub,
            plant_instances,
            plant_lod_instances: (0..registry.species.len())
                .map(|_| HashMap::new())
                .collect(),
            plant_dead_instances: (0..registry.species.len())
                .map(|_| HashMap::new())
                .collect(),
            house_instances: HashMap::new(),
            sign_instances: HashMap::new(),
        }
    }

    /// Build (or rebuild) the per-species prototype meshes into `models`.
    ///
    /// `kind == "shrub"` species route to a single hardcoded crossed-quad
    /// billboard (same mesh for near, far, and dead — dead shrubs are the same
    /// card under the weathered tint); all other species get the full
    /// procedural mesh, a simplified LOD mesh, and a leafless dead-snag mesh.
    /// Returns `(species_names, species_lod_names, species_dead_names,
    /// species_is_shrub)`.
    fn build_species_meshes(
        device: &wgpu::Device,
        models: &mut ModelRegistry,
        registry: &PlantRegistry,
    ) -> (Vec<String>, Vec<String>, Vec<String>, Vec<bool>) {
        let mut species_names = Vec::with_capacity(registry.species.len());
        let mut species_lod_names = Vec::with_capacity(registry.species.len());
        let mut species_dead_names = Vec::with_capacity(registry.species.len());
        let mut species_is_shrub = Vec::with_capacity(registry.species.len());

        for (i, species) in registry.species.iter().enumerate() {
            if species.kind == "shrub" {
                // Hardcoded billboard, sized to the species' natural height.
                let key = format!("shrub-billboard-{}", species.name);
                let ref_height = (species.height_range[0] + species.height_range[1]) * 0.5;
                let (verts, indices) = shrub_billboard_mesh(ref_height, ref_height * 0.5);
                let mesh = upload_prototype(device, &verts, &indices, &key);
                models.models.insert(key.clone(), mesh);
                // Near, far, and dead all use the same minimal billboard.
                species_names.push(key.clone());
                species_lod_names.push(key.clone());
                species_dead_names.push(key);
                species_is_shrub.push(true);
                continue;
            }

            let key = format!("plant-{}", species.name);
            let plant_mesh = plant_gen::generate_plant_mesh(&species.species_config, i as u32);
            let plant_indices = plant_mesh.opaque_indices();
            let verts: Vec<Vertex> = plant_mesh
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: v.color,
                })
                .collect();
            let mesh = upload_prototype(device, &verts, &plant_indices, &key);
            models.models.insert(key.clone(), mesh);
            species_names.push(key);

            // LOD version with reduced complexity
            let lod_key = format!("plant-{}-lod", species.name);
            let lod_config = species.species_config.simplify_for_lod();
            let lod_plant_mesh = plant_gen::generate_plant_mesh(&lod_config, i as u32);
            let lod_indices = lod_plant_mesh.opaque_indices();
            let lod_verts: Vec<Vertex> = lod_plant_mesh
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: v.color,
                })
                .collect();
            let lod_mesh = upload_prototype(device, &lod_verts, &lod_indices, &lod_key);
            models.models.insert(lod_key.clone(), lod_mesh);
            species_lod_names.push(lod_key);

            // Dead snag: bark-only (no foliage SDF pass), so one prototype is
            // cheap enough to serve every render distance.
            let dead_key = format!("plant-{}-dead", species.name);
            let dead_config = species.species_config.deadify();
            let dead_plant_mesh = plant_gen::generate_plant_mesh(&dead_config, i as u32);
            let dead_indices = dead_plant_mesh.opaque_indices();
            let dead_verts: Vec<Vertex> = dead_plant_mesh
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: v.color,
                })
                .collect();
            let dead_mesh = upload_prototype(device, &dead_verts, &dead_indices, &dead_key);
            models.models.insert(dead_key.clone(), dead_mesh);
            species_dead_names.push(dead_key);
            species_is_shrub.push(false);
        }

        (
            species_names,
            species_lod_names,
            species_dead_names,
            species_is_shrub,
        )
    }

    /// Rebuild species prototype meshes from an updated registry.
    /// Clears all plant instance buffers so `sync_chunks` rebuilds them.
    pub fn rebuild_species(&mut self, device: &wgpu::Device, registry: &PlantRegistry) {
        self.plant_instances.clear();
        self.plant_lod_instances.clear();
        self.plant_dead_instances.clear();

        let (species_names, species_lod_names, species_dead_names, species_is_shrub) =
            Self::build_species_meshes(device, &mut self.models, registry);
        self.species_names = species_names;
        self.species_lod_names = species_lod_names;
        self.species_dead_names = species_dead_names;
        self.species_is_shrub = species_is_shrub;

        self.plant_instances = (0..registry.species.len())
            .map(|_| HashMap::new())
            .collect();
        self.plant_lod_instances = (0..registry.species.len())
            .map(|_| HashMap::new())
            .collect();
        self.plant_dead_instances = (0..registry.species.len())
            .map(|_| HashMap::new())
            .collect();
    }

    /// Retains only chunks present in `world_chunks`, builds missing instance buffers.
    pub fn sync_chunks(
        &mut self,
        device: &wgpu::Device,
        world_chunks: &HashMap<IVec2, ChunkData>,
        registry: &PlantRegistry,
    ) {
        // Retain only loaded chunks
        for per_species in &mut self.plant_instances {
            per_species.retain(|coord, _| world_chunks.contains_key(coord));
        }
        for per_species in &mut self.plant_lod_instances {
            per_species.retain(|coord, _| world_chunks.contains_key(coord));
        }
        for per_species in &mut self.plant_dead_instances {
            per_species.retain(|coord, _| world_chunks.contains_key(coord));
        }
        self.house_instances
            .retain(|coord, _| world_chunks.contains_key(coord));
        self.sign_instances
            .retain(|coord, _| world_chunks.contains_key(coord));

        for (coord, chunk) in world_chunks {
            let stale = |maps: &[HashMap<IVec2, ChunkPlantBuffers>]| {
                maps.iter().any(|chunks| {
                    chunks
                        .get(coord)
                        .map(|entry| entry.revision != chunk.content.plants_revision)
                        .unwrap_or(true)
                })
            };
            let needs_rebuild = stale(&self.plant_instances)
                || stale(&self.plant_lod_instances)
                || stale(&self.plant_dead_instances);

            if needs_rebuild {
                let per_species = build_plant_instances(&chunk.content.plants, &registry.species);
                for (i, buckets) in per_species.into_iter().enumerate() {
                    let PlantInstanceBuckets { mature, lod, dead } = buckets;
                    self.plant_instances[i].insert(
                        *coord,
                        ChunkPlantBuffers {
                            revision: chunk.content.plants_revision,
                            gpu: upload_instances(device, &mature, &self.species_names[i]),
                        },
                    );

                    self.plant_lod_instances[i].insert(
                        *coord,
                        ChunkPlantBuffers {
                            revision: chunk.content.plants_revision,
                            gpu: upload_instances(device, &lod, &self.species_lod_names[i]),
                        },
                    );

                    self.plant_dead_instances[i].insert(
                        *coord,
                        ChunkPlantBuffers {
                            revision: chunk.content.plants_revision,
                            gpu: upload_instances(device, &dead, &self.species_dead_names[i]),
                        },
                    );
                }
            }

            // Houses
            if !self.house_instances.contains_key(coord) {
                if let Some(gpu) = upload_instances(
                    device,
                    &build_house_instances(&chunk.content.houses),
                    "house",
                ) {
                    self.house_instances.insert(*coord, gpu);
                }
            }

            // Debug tile-marker sign: one post at the chunk center, on the ground.
            if !self.sign_instances.contains_key(coord) {
                let half = CHUNK_SIZE_METERS * 0.5;
                let ground = chunk.terrain.height_at_world(half, half);
                let cx = (coord.x as f32 + 0.5) * CHUNK_SIZE_METERS;
                let cz = (coord.y as f32 + 0.5) * CHUNK_SIZE_METERS;
                let instance = InstanceData {
                    position: [cx, ground, cz],
                    rotation_y: 0.0,
                    scale: [1.0, 1.0, 1.0],
                    tilt: 0.0,
                    color: [1.0, 1.0, 1.0, 1.0],
                };
                if let Some(gpu) = upload_instances(device, &[instance], "sign-post") {
                    self.sign_instances.insert(*coord, gpu);
                }
            }
        }
    }

    /// Process hot-reloaded model data. Only house is GLB-based now.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn apply_model_reloads(&mut self, device: &wgpu::Device, reloads: &[(String, Vec<u8>)]) {
        for (name, bytes) in reloads {
            match model_loader::load_glb(device, bytes, name) {
                Ok(mesh) => {
                    self.models.hot_swap(name, mesh);
                    log::info!("hot-reloaded model: {name}");

                    if name == "house" {
                        self.house_instances.clear();
                    }
                }
                Err(e) => {
                    log::warn!("failed to hot-reload model {name}: {e:#}");
                }
            }
        }
    }

    /// Render arbitrary mesh/instance pairs through the instanced pipeline.
    /// Used by the plant editor to render custom meshes outside the world's chunk system.
    pub fn render_custom<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        meshes: &[(&'a PrototypeMesh, &'a GpuInstanceChunk)],
    ) {
        pass.set_pipeline(&self.pipeline);
        for (mesh, instance) in meshes {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.set_vertex_buffer(1, instance.instance_buffer.slice(..));
            pass.draw_indexed(0..mesh.index_count, 0, 0..instance.instance_count);
        }
    }

    pub fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        frustum: &Frustum,
        camera_position: Vec3,
        shadow_bind_group: &'a wgpu::BindGroup,
    ) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(2, shadow_bind_group, &[]);
        let mut on_billboard = false;

        // Draw each species, batching by LOD level to minimise state changes
        for (i, key) in self.species_names.iter().enumerate() {
            let lod_key = &self.species_lod_names[i];

            // Shrubs render as a single minimal billboard (no LOD split): one
            // mesh, the alpha-tested pipeline, all visible instances.
            if self.species_is_shrub[i] {
                if !on_billboard {
                    pass.set_pipeline(&self.billboard_pipeline);
                    on_billboard = true;
                }
                let Some(mesh) = self.models.get(key) else {
                    continue;
                };
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for chunks in [
                    &self.plant_instances[i],
                    &self.plant_lod_instances[i],
                    &self.plant_dead_instances[i],
                ] {
                    for (coord, inst) in chunks {
                        let Some(gpu) = &inst.gpu else { continue };
                        if !frustum.is_chunk_visible(*coord) {
                            continue;
                        }
                        pass.set_vertex_buffer(1, gpu.instance_buffer.slice(..));
                        pass.draw_indexed(0..mesh.index_count, 0, 0..gpu.instance_count);
                    }
                }
                continue;
            }

            if on_billboard {
                pass.set_pipeline(&self.pipeline);
                on_billboard = false;
            }

            // Partition visible chunks into near (hi-res) and far (LOD)
            let mut near: Vec<&GpuInstanceChunk> = Vec::new();
            let mut far: Vec<&GpuInstanceChunk> = Vec::new();
            let mut forced_lod: Vec<&GpuInstanceChunk> = Vec::new();
            for (coord, inst) in &self.plant_instances[i] {
                let Some(gpu) = &inst.gpu else { continue };
                if !frustum.is_chunk_visible(*coord) {
                    continue;
                }
                let dx = (coord.x as f32 + 0.5) * CHUNK_SIZE_METERS - camera_position.x;
                let dz = (coord.y as f32 + 0.5) * CHUNK_SIZE_METERS - camera_position.z;
                let dist_sq = dx * dx + dz * dz;
                if dist_sq < LOD_THRESHOLD_SQ {
                    near.push(gpu);
                } else {
                    far.push(gpu);
                }
            }
            for (coord, inst) in &self.plant_lod_instances[i] {
                if let Some(gpu) = &inst.gpu {
                    if frustum.is_chunk_visible(*coord) {
                        forced_lod.push(gpu);
                    }
                }
            }

            // Draw near chunks with hi-res mesh
            if let Some(mesh) = self.models.get(key) {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for inst in &near {
                    pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                    pass.draw_indexed(0..mesh.index_count, 0, 0..inst.instance_count);
                }
            }

            // Draw far chunks with LOD mesh
            if let Some(mesh) = self.models.get(lod_key) {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for inst in &far {
                    pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                    pass.draw_indexed(0..mesh.index_count, 0, 0..inst.instance_count);
                }
                for inst in &forced_lod {
                    pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                    pass.draw_indexed(0..mesh.index_count, 0, 0..inst.instance_count);
                }
            }

            // Draw dead snags with the bark-only snag mesh at every distance.
            if let Some(mesh) = self.models.get(&self.species_dead_names[i]) {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for (coord, inst) in &self.plant_dead_instances[i] {
                    let Some(gpu) = &inst.gpu else { continue };
                    if !frustum.is_chunk_visible(*coord) {
                        continue;
                    }
                    pass.set_vertex_buffer(1, gpu.instance_buffer.slice(..));
                    pass.draw_indexed(0..mesh.index_count, 0, 0..gpu.instance_count);
                }
            }
        }

        // Houses (and anything after) use the opaque pipeline.
        if on_billboard {
            pass.set_pipeline(&self.pipeline);
        }

        // Draw houses
        if let Some(mesh) = self.models.get("house") {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (coord, inst) in &self.house_instances {
                if !frustum.is_chunk_visible(*coord) {
                    continue;
                }
                pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                pass.draw_indexed(0..mesh.index_count, 0, 0..inst.instance_count);
            }
        }

        // Draw debug tile-marker sign posts.
        if let Some(mesh) = self.models.get("sign-post") {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for (coord, inst) in &self.sign_instances {
                if !frustum.is_chunk_visible(*coord) {
                    continue;
                }
                pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                pass.draw_indexed(0..mesh.index_count, 0, 0..inst.instance_count);
            }
        }
    }

    /// Depth-only pass: render shadow casters from the light's point of view.
    /// No frustum culling so off-screen objects still cast shadows. Shrub
    /// billboards are skipped (alpha-tested cards would cast rectangular
    /// shadows); trees (full-res + LOD meshes) and houses cast shadows.
    /// Assumes the caller has bound group 0 (frame uniforms).
    pub fn render_depth<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_pipeline(&self.shadow_pipeline);

        for (i, key) in self.species_names.iter().enumerate() {
            if self.species_is_shrub[i] {
                continue;
            }

            // Full-res mature instances.
            if let Some(mesh) = self.models.get(key) {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for inst in self.plant_instances[i].values() {
                    if let Some(gpu) = &inst.gpu {
                        pass.set_vertex_buffer(1, gpu.instance_buffer.slice(..));
                        pass.draw_indexed(0..mesh.index_count, 0, 0..gpu.instance_count);
                    }
                }
            }

            // Forced-LOD instances (seedlings / young).
            let lod_key = &self.species_lod_names[i];
            if let Some(mesh) = self.models.get(lod_key) {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for inst in self.plant_lod_instances[i].values() {
                    if let Some(gpu) = &inst.gpu {
                        pass.set_vertex_buffer(1, gpu.instance_buffer.slice(..));
                        pass.draw_indexed(0..mesh.index_count, 0, 0..gpu.instance_count);
                    }
                }
            }

            // Dead snags.
            if let Some(mesh) = self.models.get(&self.species_dead_names[i]) {
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                for inst in self.plant_dead_instances[i].values() {
                    if let Some(gpu) = &inst.gpu {
                        pass.set_vertex_buffer(1, gpu.instance_buffer.slice(..));
                        pass.draw_indexed(0..mesh.index_count, 0, 0..gpu.instance_count);
                    }
                }
            }
        }

        // Houses.
        if let Some(mesh) = self.models.get("house") {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for inst in self.house_instances.values() {
                pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                pass.draw_indexed(0..mesh.index_count, 0, 0..inst.instance_count);
            }
        }

        // Debug tile-marker sign posts.
        if let Some(mesh) = self.models.get("sign-post") {
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            for inst in self.sign_instances.values() {
                pass.set_vertex_buffer(1, inst.instance_buffer.slice(..));
                pass.draw_indexed(0..mesh.index_count, 0, 0..inst.instance_count);
            }
        }
    }

    pub fn stats(&self) -> InstancedStats {
        let buffered_mature_plants = self
            .plant_instances
            .iter()
            .flat_map(|chunks| chunks.values())
            .filter_map(|entry| entry.gpu.as_ref())
            .map(|gpu| gpu.instance_count as usize)
            .sum();
        let buffered_lod_plants = self
            .plant_lod_instances
            .iter()
            .flat_map(|chunks| chunks.values())
            .filter_map(|entry| entry.gpu.as_ref())
            .map(|gpu| gpu.instance_count as usize)
            .sum();
        let buffered_dead_plants = self
            .plant_dead_instances
            .iter()
            .flat_map(|chunks| chunks.values())
            .filter_map(|entry| entry.gpu.as_ref())
            .map(|gpu| gpu.instance_count as usize)
            .sum();
        let buffered_house_instances = self
            .house_instances
            .values()
            .map(|entry| entry.instance_count as usize)
            .sum();

        InstancedStats {
            buffered_mature_plants,
            buffered_lod_plants,
            buffered_dead_plants,
            buffered_house_instances,
        }
    }
}
