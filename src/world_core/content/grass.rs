//! Grass tuft placement for the near-field grass layer.
//!
//! Grass is a *view layer*, not chunk content: tufts are far too dense to
//! store for every loaded chunk, so the renderer generates them lazily in
//! 32 m tiles around the camera (see `renderer_wgpu::grass_pass`). This module
//! is the pure, deterministic placement half: seed + tile coordinate + terrain
//! fields in, sorted tuft list out. Nothing here is persisted and nothing
//! feeds the world `gen_key`.

use glam::{IVec2, Vec3};

use super::sampling::{estimate_slope, hash4, hash_to_unit_float, sample_field_bilinear};
use crate::world_core::biome::{classify, Biome};
use crate::world_core::chunk::{canonical_chunk, ChunkTerrain, CHUNK_SIZE_METERS};
use crate::world_core::config::BiomeConfig;
use crate::world_core::rivers::MAX_PLANTABLE_WETNESS;

/// Side length of one grass tile in metres. Divides `CHUNK_SIZE_METERS`
/// exactly so a tile never straddles chunk terrain fields.
pub const GRASS_TILE_SIZE: f32 = 32.0;

/// Grass tiles per chunk side.
pub const GRASS_TILES_PER_CHUNK: i32 = (CHUNK_SIZE_METERS / GRASS_TILE_SIZE) as i32;

/// Tuft grid spacing in metres within a tile.
const GRASS_SPACING: f32 = 0.5;

/// Distinct from the flora grids (tree=0, shrub=2000, aquatic=5000) so the
/// scatter patterns don't alias.
const SEED_OFFSET: u32 = 9000;

/// Slopes steeper than this (rise/run) stay bare — matches scree/cliff faces
/// where the terrain reads as rock even inside a grassy biome.
const MAX_SLOPE: f32 = 0.7;

/// Roots are sunk slightly below the bilinear surface sample so blades never
/// hover where the triangle mesh dips under the bilinear patch.
const ROOT_SINK: f32 = 0.04;

/// One grass tuft instance, ready for the renderer to tint and upload.
#[derive(Clone, Debug, PartialEq)]
pub struct GrassTuft {
    /// World-space root position (already sunk into the ground).
    pub position: Vec3,
    pub yaw: f32,
    pub width_scale: f32,
    pub height_scale: f32,
    /// Per-tuft hash in 0..1. Output is sorted by this key descending, so any
    /// buffer prefix is a spatially uniform subset — distance thinning is a
    /// prefix draw plus a matching shader-side fade.
    pub fade_key: f32,
    /// Dominant biome at the root, used to pick the ground tint.
    pub biome: Biome,
}

/// Generate the tufts of one grass tile. `tile` is the tile index within the
/// chunk, `0..GRASS_TILES_PER_CHUNK` on both axes. Deterministic for a given
/// (seed, canonical chunk, tile) regardless of camera path or call order.
pub fn generate_grass_tile(
    seed: u32,
    biome_config: &BiomeConfig,
    sea_level: f32,
    chunk_coord: IVec2,
    tile: IVec2,
    terrain: &ChunkTerrain,
) -> Vec<GrassTuft> {
    let total = crate::world_core::chunk::CHUNK_GRID_RESOLUTION
        * crate::world_core::chunk::CHUNK_GRID_RESOLUTION;
    if terrain.heights.len() != total
        || terrain.moisture.len() != total
        || terrain.river.len() != total
        || terrain.river_surface.len() != total
    {
        return Vec::new();
    }

    // Hash on the canonical chunk so chunks whole world-laps apart grow
    // identical grass; world positions stay on the raw coord (same convention
    // as FloraLayer).
    let canon = canonical_chunk(chunk_coord);
    let cells_per_side = (GRASS_TILE_SIZE / GRASS_SPACING) as i32;
    let tile_origin_x = tile.x as f32 * GRASS_TILE_SIZE;
    let tile_origin_z = tile.y as f32 * GRASS_TILE_SIZE;

    let mut tufts = Vec::new();

    for gz in 0..cells_per_side {
        for gx in 0..cells_per_side {
            // Cell id unique across the whole chunk so neighbouring tiles
            // never share hash streams.
            let cgx = (tile.x * cells_per_side + gx) as u32;
            let cgz = (tile.y * cells_per_side + gz) as u32;
            let cell_id = (cgx << 16) | cgz;
            let cell_hash = |salt: u32| {
                hash_to_unit_float(hash4(
                    seed.wrapping_add(SEED_OFFSET + salt),
                    canon.x as u32,
                    canon.y as u32,
                    cell_id,
                ))
            };

            let rnd = cell_hash(0);
            let local_x = tile_origin_x + (gx as f32 + cell_hash(71)) * GRASS_SPACING;
            let local_z = tile_origin_z + (gz as f32 + cell_hash(193)) * GRASS_SPACING;

            let height = sample_field_bilinear(&terrain.heights, local_x, local_z);
            if height < sea_level + 0.15 {
                continue;
            }

            // Same river guards as land flora: no grass in the channel or on
            // ground below the rendered water sheet.
            let wetness = sample_field_bilinear(&terrain.river, local_x, local_z);
            if wetness > MAX_PLANTABLE_WETNESS {
                continue;
            }
            let sheet = sample_field_bilinear(&terrain.river_surface, local_x, local_z);
            if height < sheet {
                continue;
            }

            if estimate_slope(&terrain.heights, local_x, local_z) > MAX_SLOPE {
                continue;
            }

            let moisture = sample_field_bilinear(&terrain.moisture, local_x, local_z);
            let biome = classify(height, moisture, biome_config);
            let density = grass_density(biome, moisture);
            if rnd > density {
                continue;
            }

            // Lusher (taller) grass where the ground is wetter.
            let height_scale = (0.80 + moisture * 0.45) * (0.70 + 0.60 * cell_hash(401));
            let width_scale = 0.80 + 0.40 * cell_hash(619);

            tufts.push(GrassTuft {
                position: Vec3::new(
                    chunk_coord.x as f32 * CHUNK_SIZE_METERS + local_x,
                    height - ROOT_SINK,
                    chunk_coord.y as f32 * CHUNK_SIZE_METERS + local_z,
                ),
                yaw: cell_hash(1117) * std::f32::consts::TAU,
                width_scale,
                height_scale,
                fade_key: cell_hash(2719),
                biome,
            });
        }
    }

    // Descending fade keys: any prefix of the buffer is a uniform random
    // subset, so the renderer thins with distance by drawing fewer instances.
    tufts.sort_by(|a, b| b.fade_key.total_cmp(&a.fade_key));
    tufts
}

/// Per-cell tuft probability. Grassland reads as a continuous lawn, forest as
/// a sparser under-story; deserts keep occasional dry tufts (they pick up the
/// sandy ground tint automatically). Rock and snow stay bare.
fn grass_density(biome: Biome, moisture: f32) -> f32 {
    match biome {
        Biome::Grassland => (0.55 + moisture * 0.40).min(0.92),
        Biome::Forest => 0.30,
        Biome::Desert => 0.04,
        Biome::Rock | Biome::Snow => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_core::chunk::CHUNK_GRID_RESOLUTION;

    fn flat_terrain(height: f32, moisture: f32) -> ChunkTerrain {
        let total = CHUNK_GRID_RESOLUTION * CHUNK_GRID_RESOLUTION;
        ChunkTerrain {
            heights: vec![height; total],
            moisture: vec![moisture; total],
            river: vec![0.0; total],
            river_flow: vec![[0.0, 0.0]; total],
            river_surface: vec![0.0; total],
            min_height: height,
            max_height: height,
            has_water: false,
            has_river: false,
        }
    }

    const SEA_LEVEL: f32 = 40.0;

    fn gen(terrain: &ChunkTerrain) -> Vec<GrassTuft> {
        generate_grass_tile(
            42,
            &BiomeConfig::default(),
            SEA_LEVEL,
            IVec2::new(3, -2),
            IVec2::new(4, 5),
            terrain,
        )
    }

    #[test]
    fn deterministic_for_same_inputs() {
        let terrain = flat_terrain(80.0, 0.5);
        assert_eq!(gen(&terrain), gen(&terrain));
        assert!(!gen(&terrain).is_empty());
    }

    #[test]
    fn no_grass_below_sea_level() {
        let terrain = flat_terrain(SEA_LEVEL - 5.0, 0.5);
        assert!(gen(&terrain).is_empty());
    }

    #[test]
    fn no_grass_in_river_channels() {
        let mut terrain = flat_terrain(80.0, 0.5);
        terrain.river = vec![MAX_PLANTABLE_WETNESS * 2.0; terrain.river.len()];
        assert!(gen(&terrain).is_empty());
    }

    #[test]
    fn grassland_is_denser_than_desert_and_snow_is_bare() {
        let grassland = gen(&flat_terrain(80.0, 0.5)).len();
        let desert = gen(&flat_terrain(80.0, 0.1)).len();
        let snow = gen(&flat_terrain(200.0, 0.5)).len();
        assert!(grassland > desert * 5, "{grassland} vs {desert}");
        assert_eq!(snow, 0);
    }

    #[test]
    fn sorted_by_fade_key_descending_and_positions_in_tile() {
        let terrain = flat_terrain(80.0, 0.5);
        let tufts = gen(&terrain);
        for pair in tufts.windows(2) {
            assert!(pair[0].fade_key >= pair[1].fade_key);
        }
        let min_x = 3.0 * CHUNK_SIZE_METERS + 4.0 * GRASS_TILE_SIZE;
        let min_z = -2.0 * CHUNK_SIZE_METERS + 5.0 * GRASS_TILE_SIZE;
        for t in &tufts {
            assert!(t.position.x >= min_x && t.position.x <= min_x + GRASS_TILE_SIZE + 1.0);
            assert!(t.position.z >= min_z && t.position.z <= min_z + GRASS_TILE_SIZE + 1.0);
            assert!((t.position.y - (80.0 - ROOT_SINK)).abs() < 1e-4);
        }
    }
}
