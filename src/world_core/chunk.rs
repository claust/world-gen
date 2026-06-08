use glam::{IVec2, Vec3};

use crate::world_core::lifecycle::GrowthStage;

pub const CHUNK_SIZE_METERS: f32 = 256.0;
pub const CHUNK_GRID_RESOLUTION: usize = 129;

/// Side length of the world in chunks. The world is a finite, seamlessly
/// wrapping flat plane (a torus): chunk coordinates wrap every
/// `WORLD_SIZE_CHUNKS` along each axis, so there are only `WORLD_SIZE_CHUNKS²`
/// distinct chunks. World side `L = WORLD_SIZE_CHUNKS * CHUNK_SIZE_METERS`
/// (= 65 536 m at the default).
///
/// This is part of the world format: changing it shifts every canonical chunk
/// id and the noise period, invalidating existing saves. Must stay comfortably
/// larger than the streaming load diameter (`2 * load_radius + 1`) so a single
/// canonical chunk never appears at two raw positions in one frame.
pub const WORLD_SIZE_CHUNKS: i32 = 256;

/// World side length in metres — the period of the wrapping terrain noise.
pub const WORLD_SIZE_METERS: f64 = WORLD_SIZE_CHUNKS as f64 * CHUNK_SIZE_METERS as f64;

/// Global water surface height. Any terrain below this level is submerged.
pub const SEA_LEVEL: f32 = 40.0;

/// Minimum per-vertex river wetness for a vertex to count as carrying flowing
/// water. Below this the riverbed tint fades out and no water surface is drawn,
/// so it doubles as the cutoff for `ChunkTerrain::has_river` and the river mesh.
pub const RIVER_SURFACE_THRESHOLD: f32 = 0.04;

/// Lift of the river water surface above the carved bed, in metres. The sheet
/// hugs the bed at the banks (low wetness) and rises toward the channel centre
/// (high wetness), so deeper channels fill more and the surface self-levels.
/// Shared between the rendered water surface (`RiverPass`) and the cattail
/// spawn gate (`FloraLayer`) so the two can never disagree on how deep the
/// water is at a given wetness.
pub const RIVER_MIN_DEPTH: f32 = 0.35;
pub const RIVER_DEPTH_SCALE: f32 = 6.0;

/// Freeboard (metres) by which the river water sheet sits below the smoothed
/// routing surface (the pit-filled hydrology field `RiverField::surf`). The
/// rendered surface is this single, locally flat sheet — not a per-vertex lift
/// off the bumpy carved bed — so a river reads as level water filling a channel
/// instead of a sheet draped over the bed's cross-section. Larger = water sits
/// lower in the channel (narrower, banks more exposed); smaller = fuller, wider.
pub const RIVER_SURFACE_FREEBOARD: f32 = 4.0;

/// Depth of river water above the carved bed for a given wetness, in metres.
/// Returns 0 below `RIVER_SURFACE_THRESHOLD`, where no surface is drawn.
pub fn river_water_depth(wetness: f32) -> f32 {
    if wetness > RIVER_SURFACE_THRESHOLD {
        RIVER_MIN_DEPTH + wetness * RIVER_DEPTH_SCALE
    } else {
        0.0
    }
}

/// Map any (possibly unbounded) raw chunk coordinate to its canonical id in
/// `[0, WORLD_SIZE_CHUNKS)` on both axes. Raw chunk ids an integer number of
/// laps apart collapse to the same canonical id, so they share generated
/// terrain, vegetation, and persisted state — the wrap is invisible because
/// streaming, culling, and placement keep using the raw id, while content
/// generation and lookup go through the canonical id.
pub fn canonical_chunk(coord: IVec2) -> IVec2 {
    IVec2::new(
        coord.x.rem_euclid(WORLD_SIZE_CHUNKS),
        coord.y.rem_euclid(WORLD_SIZE_CHUNKS),
    )
}

/// Rebase a stored world position into the world span of `raw_coord`.
///
/// Persisted plant state (the delta layer) is keyed by *canonical* chunk and so
/// is shared by every raw chunk a whole number of world laps apart. Its plant
/// positions are absolute, frozen at whatever raw location they were created.
/// When that canonical chunk is later loaded at a different raw position (e.g.
/// after flying a full lap), this maps each stored position into the loaded
/// chunk's lap so it lines up with the raw-positioned base plants and renders
/// next to the camera instead of a lap away.
///
/// Reduces the position modulo the world side to its canonical span, then adds
/// the loaded chunk's lap offset. Idempotent: rebasing an already-correct
/// position is a no-op.
pub fn rebase_position_to_chunk(position: Vec3, raw_coord: IVec2) -> Vec3 {
    let world = WORLD_SIZE_METERS as f32;
    let canon = canonical_chunk(raw_coord);
    let lap_x = (raw_coord.x - canon.x) as f32 * CHUNK_SIZE_METERS;
    let lap_z = (raw_coord.y - canon.y) as f32 * CHUNK_SIZE_METERS;
    Vec3::new(
        position.x.rem_euclid(world) + lap_x,
        position.y,
        position.z.rem_euclid(world) + lap_z,
    )
}

#[derive(Clone, PartialEq)]
pub struct PlantInstance {
    pub position: Vec3,
    pub rotation: f32,
    pub height: f32,
    pub species_index: usize,
    pub growth_stage: GrowthStage,
}

#[derive(Clone)]
pub struct HouseInstance {
    pub position: Vec3,
    pub rotation: f32,
}

#[derive(Clone, Default)]
pub struct ChunkContent {
    pub base_plants: Vec<PlantInstance>,
    pub plants: Vec<PlantInstance>,
    pub plants_revision: u64,
    pub houses: Vec<HouseInstance>,
}

impl ChunkContent {
    pub fn set_plants(&mut self, plants: Vec<PlantInstance>) -> bool {
        if self.plants == plants {
            return false;
        }

        self.plants = plants;
        self.plants_revision = self.plants_revision.wrapping_add(1);
        true
    }
}

#[derive(Clone)]
pub struct ChunkTerrain {
    pub heights: Vec<f32>,
    pub moisture: Vec<f32>,
    /// Per-vertex river wetness in `0..1`, parallel to `heights`. Drives the
    /// riverbed tint in the terrain shader; `0` everywhere when rivers are off.
    pub river: Vec<f32>,
    /// Per-vertex normalized downstream flow direction in world space (x, z),
    /// parallel to `heights`. `(0, 0)` away from rivers. Drives the streaming
    /// animation of the river water surface.
    pub river_flow: Vec<[f32; 2]>,
    /// Per-vertex river water-surface elevation in metres, parallel to `heights`
    /// (the smoothed routing surface minus [`RIVER_SURFACE_FREEBOARD`]). This is
    /// a locally *flat* sheet, independent of the bumpy carved bed, so the
    /// rendered water is level across the channel. The water depth at a vertex is
    /// `river_surface - heights`; where that is negative the bed pokes above the
    /// sheet and the shader discards the fragment, giving a crisp shoreline.
    pub river_surface: Vec<f32>,
    pub min_height: f32,
    pub max_height: f32,
    /// `true` when any vertex in this chunk is below `SEA_LEVEL`.
    pub has_water: bool,
    /// `true` when any vertex in this chunk carries meaningful river wetness, so
    /// the renderer can skip building a river water surface for dry chunks.
    pub has_river: bool,
}

impl ChunkTerrain {
    /// Sample the terrain height at a position local to this chunk, where
    /// `local_x`/`local_z` are in meters within `[0, CHUNK_SIZE_METERS]`.
    /// Bilinearly interpolates the stored `CHUNK_GRID_RESOLUTION²` height grid.
    pub fn height_at_world(&self, local_x: f32, local_z: f32) -> f32 {
        let side = CHUNK_GRID_RESOLUTION;
        let cell = CHUNK_SIZE_METERS / (side - 1) as f32;

        let max_idx = (side - 1) as f32;
        let fx = (local_x / cell).clamp(0.0, max_idx);
        let fz = (local_z / cell).clamp(0.0, max_idx);

        let x0 = fx.floor() as usize;
        let z0 = fz.floor() as usize;
        let x1 = (x0 + 1).min(side - 1);
        let z1 = (z0 + 1).min(side - 1);
        let tx = fx - x0 as f32;
        let tz = fz - z0 as f32;

        let h = |x: usize, z: usize| self.heights[z * side + x];
        let top = h(x0, z0) * (1.0 - tx) + h(x1, z0) * tx;
        let bottom = h(x0, z1) * (1.0 - tx) + h(x1, z1) * tx;
        top * (1.0 - tz) + bottom * tz
    }
}

#[derive(Clone)]
pub struct ChunkData {
    pub coord: IVec2,
    pub terrain: ChunkTerrain,
    pub content: ChunkContent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebase_position_is_a_noop_within_the_canonical_span() {
        // A position already in its canonical chunk's span is unchanged.
        let coord = IVec2::new(3, 7);
        let pos = Vec3::new(
            coord.x as f32 * CHUNK_SIZE_METERS + 12.0,
            80.0,
            coord.y as f32 * CHUNK_SIZE_METERS + 200.0,
        );
        let rebased = rebase_position_to_chunk(pos, coord);
        assert!((rebased - pos).length() < 1e-3);
    }

    #[test]
    fn rebase_position_shifts_a_stored_position_into_the_loaded_lap() {
        // A delta plant created at canonical chunk (3, 7), viewed at the same
        // canonical chunk one full world lap east, lands one lap further along x
        // and keeps its chunk-local offset and height.
        let canon = IVec2::new(3, 7);
        let stored = Vec3::new(
            canon.x as f32 * CHUNK_SIZE_METERS + 12.0,
            80.0,
            canon.y as f32 * CHUNK_SIZE_METERS + 200.0,
        );
        let raw = IVec2::new(canon.x + WORLD_SIZE_CHUNKS, canon.y);
        let lap = WORLD_SIZE_CHUNKS as f32 * CHUNK_SIZE_METERS;

        let rebased = rebase_position_to_chunk(stored, raw);

        assert!((rebased.x - (stored.x + lap)).abs() < 1e-2);
        assert!((rebased.z - stored.z).abs() < 1e-2);
        assert!((rebased.y - stored.y).abs() < 1e-3);
        assert_eq!(canonical_chunk(world_chunk_of(rebased)), canon);
    }

    #[test]
    fn rebase_position_is_idempotent() {
        let raw = IVec2::new(-1, 4);
        let stored = Vec3::new(-244.0, 50.0, 1080.0);
        let once = rebase_position_to_chunk(stored, raw);
        let twice = rebase_position_to_chunk(once, raw);
        assert!((once - twice).length() < 1e-3);
    }

    fn world_chunk_of(pos: Vec3) -> IVec2 {
        IVec2::new(
            (pos.x / CHUNK_SIZE_METERS).floor() as i32,
            (pos.z / CHUNK_SIZE_METERS).floor() as i32,
        )
    }
}
