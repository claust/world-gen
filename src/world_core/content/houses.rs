use glam::{IVec2, Vec3};

use super::sampling::{
    estimate_slope, hash4, hash_to_unit_float, sample_biome_nearest, sample_field_bilinear,
};
use crate::world_core::biome::Biome;
use crate::world_core::biome_map::BiomeMap;
use crate::world_core::chunk::{
    canonical_chunk, ChunkTerrain, HouseInstance, CHUNK_GRID_RESOLUTION, CHUNK_SIZE_METERS,
};
use crate::world_core::config::HousesConfig;
use crate::world_core::layer::Layer;

pub struct HousesLayer {
    seed: u32,
    config: HousesConfig,
    sea_level: f32,
}

pub struct HousesInput<'a> {
    pub coord: IVec2,
    pub terrain: &'a ChunkTerrain,
    pub biome_map: &'a BiomeMap,
}

/// One placed house, independent of how the site was sampled: world XZ position,
/// ground height, and facing. The renderer turns this into a [`HouseInstance`];
/// the live spread pass keeps only the XZ to fence seedlings away from houses.
pub(crate) struct HouseSite {
    pub world_x: f32,
    pub world_z: f32,
    pub height: f32,
    pub rotation: f32,
}

/// Emit a chunk's houses, hashing on the canonical (wrapped) chunk id so chunks a
/// whole number of world laps apart place the same hamlets/houses, while world
/// positions stay on the raw `coord`.
///
/// `validate(local_x, local_z)` returns the ground height when a site is
/// buildable (grassland, slope/height in range, above sea level) or `None`
/// otherwise. Sharing this loop lets chunk rendering validate against the
/// per-chunk terrain grid while the spread pass validates against the continuous
/// heightmap — one source of truth for *where* houses can stand, so the two never
/// disagree about a house's location.
pub(crate) fn place_houses<F>(
    seed: u32,
    config: &HousesConfig,
    coord: IVec2,
    mut validate: F,
) -> Vec<HouseSite>
where
    F: FnMut(f32, f32) -> Option<f32>,
{
    let canon = canonical_chunk(coord);
    let cx = canon.x as u32;
    let cy = canon.y as u32;
    let origin_x = coord.x as f32 * CHUNK_SIZE_METERS;
    let origin_z = coord.y as f32 * CHUNK_SIZE_METERS;
    let mut houses = Vec::new();

    // A hamlet house may be jittered past the chunk edge; reject it rather than
    // letting the grid sampler silently clamp it back in-bounds.
    let in_bounds =
        |x: f32, z: f32| x >= 0.0 && z >= 0.0 && x < CHUNK_SIZE_METERS && z < CHUNK_SIZE_METERS;
    let mut validate_local = |x: f32, z: f32| -> Option<f32> {
        if in_bounds(x, z) {
            validate(x, z)
        } else {
            None
        }
    };

    // Phase 1: Hamlet clusters on a coarse grid
    let hamlet_spacing = config.hamlet_spacing;
    if hamlet_spacing.is_finite() && hamlet_spacing > 0.0 {
        let hamlet_cells = (CHUNK_SIZE_METERS / hamlet_spacing) as i32;
        let min_houses = config.hamlet_house_min.min(config.hamlet_house_max);
        let max_houses = config.hamlet_house_min.max(config.hamlet_house_max);
        let range = max_houses.saturating_sub(min_houses).saturating_add(1);

        for gz in 0..hamlet_cells {
            for gx in 0..hamlet_cells {
                let cell_id = ((gx as u32) << 16) | (gz as u32);

                let rnd = hash_to_unit_float(hash4(seed.wrapping_add(3001), cx, cy, cell_id));
                if rnd > config.hamlet_density {
                    continue;
                }

                // Hamlet center position (with jitter)
                let jx = hash_to_unit_float(hash4(seed.wrapping_add(3071), cx, cy, cell_id));
                let jz = hash_to_unit_float(hash4(seed.wrapping_add(3093), cx, cy, cell_id));
                let center_x = (gx as f32 + jx) * hamlet_spacing;
                let center_z = (gz as f32 + jz) * hamlet_spacing;

                if validate_local(center_x, center_z).is_none() {
                    continue;
                }

                let count_rnd = hash4(seed.wrapping_add(3201), cx, cy, cell_id);
                let count = min_houses + (count_rnd % range);

                for i in 0..count {
                    let sub_id = cell_id.wrapping_mul(31).wrapping_add(i);

                    let angle = hash_to_unit_float(hash4(seed.wrapping_add(3301), cx, cy, sub_id))
                        * std::f32::consts::TAU;

                    // sqrt for uniform disk distribution
                    let raw_dist =
                        hash_to_unit_float(hash4(seed.wrapping_add(3401), cx, cy, sub_id));
                    let dist = raw_dist.sqrt() * config.hamlet_radius;

                    let local_x = center_x + angle.cos() * dist;
                    let local_z = center_z + angle.sin() * dist;

                    let height = match validate_local(local_x, local_z) {
                        Some(h) => h,
                        None => continue,
                    };

                    let rotation =
                        hash_to_unit_float(hash4(seed.wrapping_add(3501), cx, cy, sub_id))
                            * std::f32::consts::TAU;

                    houses.push(HouseSite {
                        world_x: origin_x + local_x,
                        world_z: origin_z + local_z,
                        height,
                        rotation,
                    });
                }
            }
        }
    }

    // Phase 2: Solo houses on the fine grid (lower density)
    let spacing = config.grid_spacing;
    let cells_per_side = (CHUNK_SIZE_METERS / spacing) as i32;

    for gz in 0..cells_per_side {
        for gx in 0..cells_per_side {
            let cell_id = ((gx as u32) << 16) | (gz as u32);
            let rnd = hash_to_unit_float(hash4(seed.wrapping_add(1001), cx, cy, cell_id));

            let jitter_x = hash_to_unit_float(hash4(seed.wrapping_add(1071), cx, cy, cell_id));
            let jitter_z = hash_to_unit_float(hash4(seed.wrapping_add(1193), cx, cy, cell_id));

            let local_x = (gx as f32 + jitter_x) * spacing;
            let local_z = (gz as f32 + jitter_z) * spacing;

            // Solo houses exist only in grassland, at `grassland_density`. The old
            // code derived the density from a separate biome sample (0 outside
            // grassland); since `validate` already rejects every non-grassland
            // site, gating on the constant here is equivalent and the produced set
            // is identical.
            if rnd > config.grassland_density {
                continue;
            }

            let height = match validate_local(local_x, local_z) {
                Some(h) => h,
                None => continue,
            };

            let rotation = hash_to_unit_float(hash4(seed.wrapping_add(1401), cx, cy, cell_id))
                * std::f32::consts::TAU;

            houses.push(HouseSite {
                world_x: origin_x + local_x,
                world_z: origin_z + local_z,
                height,
                rotation,
            });
        }
    }

    houses
}

impl HousesLayer {
    pub fn new(seed: u32, config: HousesConfig, sea_level: f32) -> Self {
        Self {
            seed,
            config,
            sea_level,
        }
    }
}

impl<'a> Layer<HousesInput<'a>, Vec<HouseInstance>> for HousesLayer {
    fn generate(&self, input: HousesInput<'a>) -> Vec<HouseInstance> {
        let terrain = input.terrain;
        let biome_map = input.biome_map;

        let total = CHUNK_GRID_RESOLUTION * CHUNK_GRID_RESOLUTION;
        if terrain.heights.len() != total
            || terrain.moisture.len() != total
            || biome_map.values.len() != total
        {
            return Vec::new();
        }

        // Rendering path: validate each candidate against the per-chunk terrain
        // grid, in chunk-local coordinates.
        place_houses(self.seed, &self.config, input.coord, |local_x, local_z| {
            if sample_biome_nearest(&biome_map.values, local_x, local_z) != Biome::Grassland {
                return None;
            }
            let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
            let slope = estimate_slope(&terrain.heights, local_x, local_z);
            if slope <= self.config.max_slope
                && height >= self.sea_level
                && (self.config.height_min..=self.config.height_max).contains(&height)
            {
                Some(height)
            } else {
                None
            }
        })
        .into_iter()
        .map(|site| HouseInstance {
            position: Vec3::new(site.world_x, site.height, site.world_z),
            rotation: site.rotation,
        })
        .collect()
    }
}
