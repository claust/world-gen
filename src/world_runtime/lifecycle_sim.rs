//! Loaded-only spread + catch-up simulation.
//!
//! Superseded by the global [`PlantWorld`](super::plant_world::PlantWorld) tick
//! (M2): the render bridge now reads plants from `PlantWorld`, so the runtime no
//! longer drives this path. It is kept dormant (still compiling and test-covered)
//! until M3 reintroduces spread globally and deletes this module together with
//! the catch-up machinery — keeping that a clean, reviewable diff.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use glam::{IVec2, Vec3};

use super::delta_store::DeltaStore;
use super::plant_landing::{
    existing_plants_for_chunk, validate_seedling_landing, PlantLandingRules,
};
use super::streaming::world_to_chunk;
use crate::world_core::chunk::{canonical_chunk, ChunkData};
use crate::world_core::content::sampling::{hash4, hash_to_unit_float};
use crate::world_core::herbarium::PlantRegistry;
use crate::world_core::lifecycle::{
    advance_delta_plant_growth, assemble_plants, ChunkDelta, DeltaPlant, GrowthStage,
    MAX_CATCH_UP_HOURS,
};

pub(super) struct LifecycleTickContext<'a> {
    pub(super) loaded: &'a HashMap<IVec2, ChunkData>,
    pub(super) total_hours: f64,
    pub(super) world_seed: u32,
    pub(super) landing_rules: PlantLandingRules<'a>,
}

pub(super) struct SeedlingSpawnRequest {
    pub(super) coord: IVec2,
    pub(super) plant_index: u32,
    pub(super) seed_i: u32,
    pub(super) source_position: Vec3,
    pub(super) species_index: usize,
    pub(super) round_hour: f64,
}

pub(super) fn tick_chunk_lifecycle(
    coord: IVec2,
    context: &LifecycleTickContext<'_>,
    delta_store: &mut DeltaStore,
    changed_coords: &mut HashSet<IVec2>,
) {
    let Some(chunk) = context.loaded.get(&coord) else {
        return;
    };

    // Spread randomness is hashed on the canonical chunk id so a chunk spreads
    // identically no matter which raw lap it is loaded at; positions below stay
    // in raw world space (from the source plant) and get rebased on load.
    let canon = canonical_chunk(coord);

    let last_sim_hour = {
        let delta = delta_store.get_or_create(coord);
        sanitize_chunk_timestamps(coord, context.total_hours, delta)
    };

    let current_hour = context.total_hours.max(last_sim_hour);
    let target_hour = current_hour.min(last_sim_hour + MAX_CATCH_UP_HOURS);
    let missed_boundaries =
        (target_hour.floor() as i64 - last_sim_hour.floor() as i64).max(0) as u64;
    if missed_boundaries == 0 {
        return;
    }

    if target_hour < current_hour {
        log::warn!(
            "clamped lifecycle catch-up for chunk {coord:?} from {:.2}h to {:.2}h (cap {:.2}h)",
            context.total_hours - last_sim_hour,
            target_hour - last_sim_hour,
            MAX_CATCH_UP_HOURS
        );
    }

    for boundary in 1..=missed_boundaries {
        let round_hour = last_sim_hour.floor() + boundary as f64;
        let mut chunk_changed = false;
        {
            let delta = delta_store.get_or_create(coord);
            let previous_stages: Vec<_> =
                delta.added_plants.iter().map(|plant| plant.stage).collect();

            for plant in &delta.added_plants {
                debug_assert!(
                    plant.born_hour <= context.total_hours,
                    "chunk {coord:?} has plant born in the future: {} > {}",
                    plant.born_hour,
                    context.total_hours,
                );
            }

            for plant in &mut delta.added_plants {
                if advance_delta_plant_growth(plant, round_hour, context.landing_rules.registry) {
                    chunk_changed = true;
                }
            }

            debug_assert!(
                delta
                    .added_plants
                    .iter()
                    .zip(previous_stages.iter())
                    .all(|(plant, previous)| plant.stage >= *previous),
                "chunk {coord:?} contained a regressed growth stage"
            );

            if delta.last_sim_hour != round_hour {
                delta.last_sim_hour = round_hour;
            }
        }

        if is_spread_hour(round_hour) {
            let source_delta = delta_store.get(&coord).cloned().unwrap_or_default();
            let source_plants = assemble_plants(&chunk.content.base_plants, &source_delta);

            for (plant_index, plant) in source_plants.iter().enumerate() {
                if plant.growth_stage != GrowthStage::Mature {
                    continue;
                }

                let Some(species) = context
                    .landing_rules
                    .registry
                    .species
                    .get(plant.species_index)
                else {
                    debug_assert!(
                        false,
                        "plant in chunk {coord:?} references invalid species index {}",
                        plant.species_index
                    );
                    continue;
                };

                if spread_roll(context.world_seed, canon, plant_index as u32)
                    >= species.placement.spread_chance.clamp(0.0, 1.0)
                {
                    continue;
                }

                let seed_count = spread_seed_count(context.world_seed, canon, plant_index as u32);
                for seed_i in 0..seed_count {
                    let request = SeedlingSpawnRequest {
                        coord: canon,
                        plant_index: plant_index as u32,
                        seed_i,
                        source_position: plant.position,
                        species_index: plant.species_index,
                        round_hour,
                    };
                    let Some(seedling) = spawn_seedling(
                        context.world_seed,
                        &request,
                        context.landing_rules.registry,
                    ) else {
                        continue;
                    };

                    let target_coord = world_to_chunk(seedling.position);
                    if let Some(target_chunk) = context.loaded.get(&target_coord) {
                        let existing =
                            existing_plants_for_chunk(target_chunk, delta_store.get(&target_coord));
                        let Some(seedling) = validate_seedling_landing(
                            &seedling,
                            target_coord,
                            target_chunk,
                            &existing,
                            &context.landing_rules,
                        ) else {
                            continue;
                        };

                        delta_store
                            .get_or_create(target_coord)
                            .added_plants
                            .push(seedling);
                        changed_coords.insert(target_coord);
                    } else {
                        delta_store
                            .get_or_create(target_coord)
                            .added_plants
                            .push(seedling);
                    }
                    if target_coord == coord {
                        chunk_changed = true;
                    }
                }
            }
        }

        if chunk_changed {
            changed_coords.insert(coord);
        }
    }

    let final_delta = delta_store.get_or_create(coord);
    debug_assert!(
        final_delta.last_sim_hour <= context.total_hours,
        "chunk {coord:?} last_sim_hour {} exceeds total_hours {}",
        final_delta.last_sim_hour,
        context.total_hours
    );
}

fn sanitize_chunk_timestamps(coord: IVec2, total_hours: f64, delta: &mut ChunkDelta) -> f64 {
    if delta.last_sim_hour > total_hours {
        log::warn!(
            "clamping chunk {coord:?} last_sim_hour from {:.3}h down to current total_hours {:.3}h",
            delta.last_sim_hour,
            total_hours
        );
        delta.last_sim_hour = total_hours;
    }

    for plant in &mut delta.added_plants {
        if plant.born_hour > total_hours {
            log::warn!(
                "clamping future-born plant in chunk {coord:?} from {:.3}h down to current total_hours {:.3}h",
                plant.born_hour,
                total_hours
            );
            plant.born_hour = total_hours;
        }
    }

    debug_assert!(
        delta.last_sim_hour <= total_hours,
        "chunk {coord:?} last_sim_hour {} exceeds total_hours {total_hours}",
        delta.last_sim_hour
    );
    debug_assert!(
        delta
            .added_plants
            .iter()
            .all(|plant| plant.born_hour <= total_hours),
        "chunk {coord:?} contains plants born after total_hours {total_hours}"
    );

    delta.last_sim_hour
}

fn is_spread_hour(hour: f64) -> bool {
    (hour.floor() as i64).rem_euclid(24) == 0
}

pub(super) fn spread_roll(seed: u32, coord: IVec2, plant_index: u32) -> f32 {
    hash_to_unit_float(hash4(
        seed.wrapping_add(4001),
        coord.x as u32,
        coord.y as u32,
        plant_index,
    ))
}

fn spread_seed_count(seed: u32, coord: IVec2, plant_index: u32) -> u32 {
    1 + (hash_to_unit_float(hash4(
        seed.wrapping_add(4002),
        coord.x as u32,
        coord.y as u32,
        plant_index,
    )) * 2.0)
        .floor() as u32
}

pub(super) fn spawn_seedling(
    seed: u32,
    request: &SeedlingSpawnRequest,
    registry: &PlantRegistry,
) -> Option<DeltaPlant> {
    let species = registry.species.get(request.species_index)?;
    let sub_id = request
        .plant_index
        .wrapping_mul(31)
        .wrapping_add(request.seed_i);
    let angle = hash_to_unit_float(hash4(
        seed.wrapping_add(4101),
        request.coord.x as u32,
        request.coord.y as u32,
        sub_id,
    )) * std::f32::consts::TAU;
    let distance = hash_to_unit_float(hash4(
        seed.wrapping_add(4102),
        request.coord.x as u32,
        request.coord.y as u32,
        sub_id,
    ))
    .sqrt()
        * species.placement.spread_radius.max(0.0);
    let height = species.height_range[0]
        + hash_to_unit_float(hash4(
            seed.wrapping_add(4201),
            request.coord.x as u32,
            request.coord.y as u32,
            sub_id,
        )) * (species.height_range[1] - species.height_range[0]);
    let rotation = hash_to_unit_float(hash4(
        seed.wrapping_add(4202),
        request.coord.x as u32,
        request.coord.y as u32,
        sub_id,
    )) * std::f32::consts::TAU;

    Some(DeltaPlant {
        position: Vec3::new(
            request.source_position.x + angle.cos() * distance,
            request.source_position.y,
            request.source_position.z + angle.sin() * distance,
        ),
        rotation,
        height,
        species_index: request.species_index,
        stage: GrowthStage::Seedling,
        born_hour: request.round_hour,
    })
}
