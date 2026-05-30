use glam::IVec2;

use crate::world_core::biome::{classify, Biome};
use crate::world_core::chunk::{ChunkData, PlantInstance, CHUNK_SIZE_METERS};
use crate::world_core::content::sampling::{estimate_slope, sample_field_bilinear};
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::lifecycle::{assemble_plants, ChunkDelta, DeltaPlant};

/// Placement rules shared by chunk loading and lifecycle spread — the single
/// "can a plant exist at this spot" gate.
pub(super) struct PlantLandingRules<'a> {
    pub(super) registry: &'a PlantRegistry,
    pub(super) biome_config: &'a crate::world_core::config::BiomeConfig,
    pub(super) sea_level: f32,
}

pub(super) fn prune_chunk_delta_on_load(
    coord: IVec2,
    chunk: &ChunkData,
    delta: &mut ChunkDelta,
    landing_rules: &PlantLandingRules<'_>,
) -> bool {
    let original_removed_len = delta.removed_base.len();
    let mut changed = delta.prune_removed_base(chunk.content.base_plants.len());
    if changed && delta.removed_base.len() != original_removed_len {
        log::info!(
            "pruned {} stale removed_base indices from chunk {coord:?} on load",
            original_removed_len - delta.removed_base.len()
        );
    }
    let mut retained = Vec::with_capacity(delta.added_plants.len());
    let mut existing = chunk.content.base_plants.clone();
    let original_added = std::mem::take(&mut delta.added_plants);
    let original_snapshot = original_added.clone();
    let original_len = original_added.len();

    for plant in original_added {
        if let Some(validated) =
            validate_seedling_landing(&plant, coord, chunk, &existing, landing_rules)
        {
            existing.push(PlantInstance {
                position: validated.position,
                rotation: validated.rotation,
                height: validated.height,
                species_index: validated.species_index,
                growth_stage: validated.stage,
            });
            retained.push(validated);
        }
    }

    if retained != original_snapshot {
        changed = true;
    }

    if retained.len() != original_len {
        log::info!(
            "pruned {} invalid deferred seedlings from chunk {coord:?} on load",
            original_len - retained.len()
        );
    }

    delta.added_plants = retained;
    changed
}

pub(super) fn validate_seedling_landing(
    seedling: &DeltaPlant,
    target_coord: IVec2,
    target_chunk: &ChunkData,
    existing: &[PlantInstance],
    landing_rules: &PlantLandingRules<'_>,
) -> Option<DeltaPlant> {
    let Some(species) = landing_rules.registry.species.get(seedling.species_index) else {
        debug_assert!(
            false,
            "seedling references invalid species index {}",
            seedling.species_index
        );
        return None;
    };

    if target_chunk.terrain.heights.is_empty() || target_chunk.terrain.moisture.is_empty() {
        return None;
    }

    let local_x = seedling.position.x - target_coord.x as f32 * CHUNK_SIZE_METERS;
    let local_z = seedling.position.z - target_coord.y as f32 * CHUNK_SIZE_METERS;
    let terrain = &target_chunk.terrain;
    let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
    let moisture = sample_field_bilinear(&terrain.moisture, local_x, local_z);
    let slope = estimate_slope(&terrain.heights, local_x, local_z);
    let biome = classify(height, moisture, landing_rules.biome_config);

    if height < landing_rules.sea_level
        || moisture < species.placement.min_moisture
        || moisture > species.placement.max_moisture
        || height < species.placement.min_altitude
        || height > species.placement.max_altitude
        || slope > species.placement.max_slope
        || !species
            .placement
            .biomes
            .iter()
            .any(|candidate| candidate == biome_name(biome))
    {
        return None;
    }

    let spacing = min_spacing_for_species(&species.kind);
    let mut landed = seedling.clone();
    landed.position.y = height;

    if existing
        .iter()
        .any(|plant| plant.position.distance(landed.position) < spacing)
    {
        return None;
    }

    Some(landed)
}

pub(super) fn existing_plants_for_chunk(
    chunk: &ChunkData,
    delta: Option<&ChunkDelta>,
) -> Vec<PlantInstance> {
    delta
        .map(|delta| assemble_plants(&chunk.content.base_plants, delta))
        .unwrap_or_else(|| chunk.content.base_plants.clone())
}

fn min_spacing_for_species(kind: &str) -> f32 {
    if kind == "shrub" {
        3.0
    } else {
        8.0
    }
}

fn biome_name(biome: Biome) -> &'static str {
    match biome {
        Biome::Forest => "Forest",
        Biome::Grassland => "Grassland",
        Biome::Desert => "Desert",
        Biome::Rock => "Rock",
        Biome::Snow => "Snow",
    }
}
