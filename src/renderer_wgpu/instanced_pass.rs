use std::collections::HashMap;

use glam::{IVec2, Vec3};

use super::frustum::Frustum;
use super::geometry::Vertex;
use super::instancing::{
    build_house_instances, build_plant_instances, upload_instances, upload_prototype,
    GpuInstanceChunk, InstanceData, ModelRegistry, PrototypeMesh,
};
#[cfg(not(target_arch = "wasm32"))]
use super::model_loader;
use super::pipeline::create_render_pipeline;
use crate::world_core::chunk::{ChunkData, CHUNK_SIZE_METERS};
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::plant_gen;

/// Squared distance (in world units) beyond which chunks use LOD meshes.
const LOD_THRESHOLD_SQ: f32 = 512.0 * 512.0;

pub struct InstancedPass {
    pipeline: wgpu::RenderPipeline,
    models: ModelRegistry,
    /// species_names[i] = model key in ModelRegistry for species i
    species_names: Vec<String>,
    /// species_lod_names[i] = LOD model key for species i
    species_lod_names: Vec<String>,
    /// Mature plants; drawn full-res nearby and as LOD at distance.
    plant_instances: Vec<HashMap<IVec2, ChunkPlantBuffers>>,
    /// Seedlings and young plants; always drawn with LOD meshes.
    plant_lod_instances: Vec<HashMap<IVec2, ChunkPlantBuffers>>,
    house_instances: HashMap<IVec2, GpuInstanceChunk>,
}

struct ChunkPlantBuffers {
    revision: u64,
    gpu: Option<GpuInstanceChunk>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InstancedStats {
    pub buffered_mature_plants: usize,
    pub buffered_lod_plants: usize,
    pub buffered_house_instances: usize,
}

impl InstancedPass {
    pub fn new(
        device: &wgpu::Device,
        render_format: wgpu::TextureFormat,
        pipeline_layout: &wgpu::PipelineLayout,
        registry: &PlantRegistry,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("instanced-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/instanced.wgsl").into()),
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
            ],
        };

        let pipeline = create_render_pipeline(
            device,
            render_format,
            pipeline_layout,
            &shader,
            &[vertex_layout, instance_layout],
            "instanced-pipeline",
        );

        let mut models = ModelRegistry::new(device);
        let mut species_names = Vec::new();
        let mut species_lod_names = Vec::new();

        // Generate procedural meshes for each species using the full plant_gen system
        for (i, species) in registry.species.iter().enumerate() {
            let key = format!("plant-{}", species.name);
            let plant_mesh = plant_gen::generate_plant_mesh(&species.species_config, i as u32);
            let verts: Vec<Vertex> = plant_mesh
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: v.color,
                })
                .collect();
            let mesh = upload_prototype(device, &verts, &plant_mesh.indices, &key);
            models.models.insert(key.clone(), mesh);
            species_names.push(key);

            // LOD version with reduced complexity
            let lod_key = format!("plant-{}-lod", species.name);
            let lod_config = species.species_config.simplify_for_lod();
            let lod_plant_mesh = plant_gen::generate_plant_mesh(&lod_config, i as u32);
            let lod_verts: Vec<Vertex> = lod_plant_mesh
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: v.color,
                })
                .collect();
            let lod_mesh = upload_prototype(device, &lod_verts, &lod_plant_mesh.indices, &lod_key);
            models.models.insert(lod_key.clone(), lod_mesh);
            species_lod_names.push(lod_key);
        }

        let plant_instances = (0..registry.species.len())
            .map(|_| HashMap::new())
            .collect();

        Self {
            pipeline,
            models,
            species_names,
            species_lod_names,
            plant_instances,
            plant_lod_instances: (0..registry.species.len())
                .map(|_| HashMap::new())
                .collect(),
            house_instances: HashMap::new(),
        }
    }

    /// Rebuild species prototype meshes from an updated registry.
    /// Clears all plant instance buffers so `sync_chunks` rebuilds them.
    pub fn rebuild_species(&mut self, device: &wgpu::Device, registry: &PlantRegistry) {
        self.species_names.clear();
        self.species_lod_names.clear();
        self.plant_instances.clear();
        self.plant_lod_instances.clear();

        for (i, species) in registry.species.iter().enumerate() {
            let key = format!("plant-{}", species.name);
            let plant_mesh = plant_gen::generate_plant_mesh(&species.species_config, i as u32);
            let verts: Vec<Vertex> = plant_mesh
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: v.color,
                })
                .collect();
            let mesh = upload_prototype(device, &verts, &plant_mesh.indices, &key);
            self.models.models.insert(key.clone(), mesh);
            self.species_names.push(key);

            let lod_key = format!("plant-{}-lod", species.name);
            let lod_config = species.species_config.simplify_for_lod();
            let lod_plant_mesh = plant_gen::generate_plant_mesh(&lod_config, i as u32);
            let lod_verts: Vec<Vertex> = lod_plant_mesh
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    color: v.color,
                })
                .collect();
            let lod_mesh = upload_prototype(device, &lod_verts, &lod_plant_mesh.indices, &lod_key);
            self.models.models.insert(lod_key.clone(), lod_mesh);
            self.species_lod_names.push(lod_key);
        }

        self.plant_instances = (0..registry.species.len())
            .map(|_| HashMap::new())
            .collect();
        self.plant_lod_instances = (0..registry.species.len())
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
        self.house_instances
            .retain(|coord, _| world_chunks.contains_key(coord));

        for (coord, chunk) in world_chunks {
            let needs_rebuild = self
                .plant_instances
                .iter()
                .zip(self.plant_lod_instances.iter())
                .any(|(mature, lod)| {
                    mature
                        .get(coord)
                        .map(|entry| entry.revision != chunk.content.plants_revision)
                        .unwrap_or(true)
                        || lod
                            .get(coord)
                            .map(|entry| entry.revision != chunk.content.plants_revision)
                            .unwrap_or(true)
                });

            if needs_rebuild {
                let per_species = build_plant_instances(&chunk.content.plants, &registry.species);
                for (i, (mature_instances, lod_instances)) in per_species.into_iter().enumerate() {
                    let label = &self.species_names[i];
                    self.plant_instances[i].insert(
                        *coord,
                        ChunkPlantBuffers {
                            revision: chunk.content.plants_revision,
                            gpu: upload_instances(device, &mature_instances, label),
                        },
                    );

                    self.plant_lod_instances[i].insert(
                        *coord,
                        ChunkPlantBuffers {
                            revision: chunk.content.plants_revision,
                            gpu: upload_instances(
                                device,
                                &lod_instances,
                                &self.species_lod_names[i],
                            ),
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
    ) {
        pass.set_pipeline(&self.pipeline);

        // Draw each species, batching by LOD level to minimise state changes
        for (i, key) in self.species_names.iter().enumerate() {
            let lod_key = &self.species_lod_names[i];

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
        let buffered_house_instances = self
            .house_instances
            .values()
            .map(|entry| entry.instance_count as usize)
            .sum();

        InstancedStats {
            buffered_mature_plants,
            buffered_lod_plants,
            buffered_house_instances,
        }
    }
}
