use serde::{Deserialize, Serialize};

use super::config::{BiomeConfig, GameConfig, HeightmapConfig, RiverConfig};
pub use super::plant_gen::config::SpeciesConfig;
use super::storage::Storage;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlacementConfig {
    pub biomes: Vec<String>,
    pub weight: f32,
    pub min_moisture: f32,
    pub max_moisture: f32,
    pub min_altitude: f32,
    pub max_altitude: f32,
    pub near_water_boost: f32,
    pub max_slope: f32,
    pub spread_radius: f32,
    pub spread_chance: f32,
    pub seedling_hours: f32,
    pub young_hours: f32,
    /// Sim-hours a plant stays `Mature` before dying (with ±25% per-plant
    /// jitter). `<= 0` means immortal — used by species that cannot respawn
    /// through the spread pass (cattails are base-only) and would otherwise go
    /// extinct.
    pub lifespan_hours: f32,
    /// Sim-hours the dead snag stands before despawning and freeing its ground.
    pub snag_hours: f32,
    /// Aquatic species (cattails) spawn *in* the river/lake margin that land
    /// plants avoid: the base-flora pass seats them in the wet band above
    /// `MAX_PLANTABLE_WETNESS` and allows them below sea level, the mirror image
    /// of the land guard. Land species leave this `false`.
    #[serde(default)]
    pub aquatic: bool,
}

#[cfg(test)]
mod tests {
    use super::{Herbarium, PlacementConfig};
    use crate::world_core::config::GameConfig;
    use crate::world_core::plant_gen::config::LeafType;

    #[test]
    fn placement_config_deserializes_missing_lifecycle_fields_with_defaults() {
        let config: PlacementConfig = serde_json::from_str(
            r#"{
                "biomes": ["Forest"],
                "weight": 1.0,
                "min_moisture": 0.2,
                "max_moisture": 0.9,
                "min_altitude": 0.0,
                "max_altitude": 100.0,
                "near_water_boost": 0.1,
                "max_slope": 0.5
            }"#,
        )
        .expect("placement config should deserialize");

        assert_eq!(config.spread_radius, 25.0);
        assert_eq!(config.spread_chance, 0.3);
        assert_eq!(config.seedling_hours, 48.0);
        assert_eq!(config.young_hours, 96.0);
        assert_eq!(config.lifespan_hours, 720.0);
        assert_eq!(config.snag_hours, 120.0);
    }

    #[test]
    fn default_seeded_herbarium_sets_lifecycle_defaults_for_all_species() {
        let herbarium = Herbarium::default_seeded();
        assert!(!herbarium.plants.is_empty());

        for entry in herbarium.plants {
            // Growth-stage durations are shared by every species.
            assert_eq!(entry.placement.seedling_hours, 48.0);
            assert_eq!(entry.placement.young_hours, 96.0);
            // Aquatic species (cattails) are base-only: they deliberately opt out
            // of the lifecycle spread pass, so the spread defaults don't apply.
            if entry.placement.aquatic {
                assert_eq!(entry.placement.spread_radius, 0.0);
                assert_eq!(entry.placement.spread_chance, 0.0);
                // Base-only species can't respawn, so they must be immortal or
                // every reed bed would eventually empty for good.
                assert!(entry.placement.lifespan_hours <= 0.0);
            } else {
                assert_eq!(entry.placement.spread_radius, 25.0);
                assert_eq!(entry.placement.spread_chance, 0.3);
                if entry.species.body_plan.kind == "shrub" {
                    assert_eq!(entry.placement.lifespan_hours, 360.0);
                    assert_eq!(entry.placement.snag_hours, 48.0);
                } else {
                    assert_eq!(entry.placement.lifespan_hours, 720.0);
                    assert_eq!(entry.placement.snag_hours, 120.0);
                }
            }
        }
    }

    #[test]
    fn generation_key_ignores_non_base_generation_config() {
        let herbarium = Herbarium::default_seeded();
        let config = GameConfig::default();
        let key = herbarium.generation_key(&config);

        let mut changed = config.clone();
        changed.world.load_radius += 2;
        changed.world.start_hour += 3.0;
        changed.world.day_speed *= 2.0;
        changed.audio.enabled = !changed.audio.enabled;
        changed.audio.master_volume = 0.25;
        changed.houses.grassland_density *= 2.0;
        changed.houses.hamlet_house_max += 4;

        assert_eq!(key, herbarium.generation_key(&changed));
    }

    #[test]
    fn generation_key_ignores_plant_rendering_traits() {
        let herbarium = Herbarium::default_seeded();
        let key = herbarium.generation_key(&GameConfig::default());

        let mut changed = herbarium.clone();
        changed.plants[0].species.color.leaf.h += 30.0;
        changed.plants[0].species.trunk.taper *= 0.5;
        changed.plants[0].species.crown.density *= 0.75;
        // leaf_type drives mesh geometry only, never base-world generation, so
        // flipping it must not move the key (Phase 1 of FOLIAGE_LEAF_TYPES.md
        // relies on this).
        let flipped = if changed.plants[0].species.foliage.leaf_type == LeafType::Needle {
            LeafType::Broadleaf
        } else {
            LeafType::Needle
        };
        changed.plants[0].species.foliage.leaf_type = flipped;

        assert_eq!(key, changed.generation_key(&GameConfig::default()));
    }

    #[test]
    fn generation_key_tracks_base_generation_inputs() {
        let herbarium = Herbarium::default_seeded();
        let config = GameConfig::default();
        let key = herbarium.generation_key(&config);

        let mut seed_changed = config.clone();
        seed_changed.world.seed += 1;
        assert_ne!(key, herbarium.generation_key(&seed_changed));

        let mut terrain_changed = config.clone();
        terrain_changed.heightmap.detail.amplitude += 1.0;
        assert_ne!(key, herbarium.generation_key(&terrain_changed));

        let mut biome_changed = config.clone();
        biome_changed.biome.forest_moisture += 0.01;
        assert_ne!(key, herbarium.generation_key(&biome_changed));

        let mut river_changed = config.clone();
        river_changed.rivers.max_carve_depth += 1.0;
        assert_ne!(key, herbarium.generation_key(&river_changed));

        let mut placement_changed = herbarium.clone();
        placement_changed.plants[0].placement.weight += 0.1;
        assert_ne!(key, placement_changed.generation_key(&config));

        let mut height_changed = herbarium.clone();
        height_changed.plants[0].species.body_plan.max_height[1] += 1.0;
        assert_ne!(key, height_changed.generation_key(&config));
    }
}

impl Default for PlacementConfig {
    fn default() -> Self {
        Self {
            biomes: vec!["Forest".to_string(), "Grassland".to_string()],
            weight: 1.0,
            min_moisture: 0.0,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 200.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
            spread_radius: 25.0,
            spread_chance: 0.3,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: TREE_LIFESPAN_HOURS,
            snag_hours: TREE_SNAG_HOURS,
            aquatic: false,
        }
    }
}

/// Default death timing per species class. Trees outlive shrubs; both spend
/// ~80% of their life mature so every lineage gets dozens of daily spread
/// rolls before the parent dies.
const TREE_LIFESPAN_HOURS: f32 = 720.0;
const TREE_SNAG_HOURS: f32 = 120.0;
const SHRUB_LIFESPAN_HOURS: f32 = 360.0;
const SHRUB_SNAG_HOURS: f32 = 48.0;
/// Immortal sentinel (`lifespan_hours <= 0`): cattails are base-only (no
/// spread), so letting them die would empty every reed bed for good.
const IMMORTAL: f32 = 0.0;

#[derive(Clone, Serialize, Deserialize)]
pub struct HerbariumEntry {
    pub name: String,
    pub species: SpeciesConfig,
    #[serde(default)]
    pub placement: PlacementConfig,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Herbarium {
    pub plants: Vec<HerbariumEntry>,
}

impl Herbarium {
    /// Deterministic 64-bit hash of the inputs that determine base-world
    /// generation. Used as the validity key for the cached base-world snapshot,
    /// so editing any generation rule invalidates a stale cache.
    ///
    /// Keep this intentionally narrower than [`GameConfig`]: audio, load radius,
    /// day/night timing, house placement, and plant rendering traits do not
    /// affect the serialized base flora, so changing them must not invalidate a
    /// large downloaded/local base cache.
    ///
    /// Deterministic *within a build*: the selected inputs serialize to a
    /// field-ordered JSON byte image (no `HashMap`s) hashed through a fixed-keyed
    /// `DefaultHasher`. It is **not** guaranteed stable across `rustc` /
    /// `serde_json` upgrades — a toolchain bump may change the key and simply
    /// invalidate the on-disk cache, which then regenerates harmlessly.
    pub fn generation_key(&self, config: &super::config::GameConfig) -> u64 {
        use std::hash::Hasher;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let inputs = BaseGenerationInputs::new(self, config);
        // A serialization failure must not silently drop an input from the key —
        // that could let a snapshot built from different inputs validate. Hash a
        // distinct marker instead (so the key changes) and warn loudly.
        match serde_json::to_vec(&inputs) {
            Ok(bytes) => hasher.write(&bytes),
            Err(e) => {
                log::warn!("base generation inputs did not serialize for the generation key: {e}");
                hasher.write(b"\0base-generation-inputs-serialize-error\0");
            }
        }
        hasher.finish()
    }
}

#[derive(Serialize)]
struct BaseGenerationInputs<'a> {
    version: u32,
    seed: u32,
    sea_level: f32,
    biome: &'a BiomeConfig,
    heightmap: &'a HeightmapConfig,
    rivers: &'a RiverConfig,
    flora: Vec<BaseGenerationPlant<'a>>,
}

#[derive(Serialize)]
struct BaseGenerationPlant<'a> {
    name: &'a str,
    kind: &'a str,
    height_range: [f32; 2],
    placement: &'a PlacementConfig,
}

impl<'a> BaseGenerationInputs<'a> {
    fn new(herbarium: &'a Herbarium, config: &'a GameConfig) -> Self {
        Self {
            // v2: terrain height's continental + ridge octaves moved to a
            // precomputed coarse global field (centimetre-scale lossy vs the
            // exact 4D noise), so the baked terrain differs and old snapshots
            // must be rejected. See `world_core::terrain_fields`.
            // v3: base flora now rejects placement in river channels (wetness >
            // `MAX_PLANTABLE_WETNESS`), so the baked plant set differs and old
            // snapshots — which still seat trees in the water — must be rejected.
            // v4: cattails (the first aquatic species) are baked into the wet
            // band along banks, adding plants old snapshots never had.
            // v5: both moisture octaves moved to the precomputed coarse global
            // field (mildly lossy bilinear vs the exact 4D noise), shifting baked
            // biome boundaries, so old snapshots must be rejected. See
            // `world_core::terrain_fields`.
            // v6: river carve is reshaped from a distance-to-channel field into
            // steep-walled, flat-bottomed channels (was a wetness-proportional
            // bowl), so the carved terrain — and the plants seated on it —
            // differ. See `world_core::rivers`. The carve-shape tunables live as
            // constants there, not in `RiverConfig`, so changing them needs a
            // further bump here.
            // v7: the distance-to-channel field now bridges diagonal channel
            // steps (it bilinearly bulged at segment midpoints, pinching the
            // carve to zero and breaking rivers into pools), so the carved
            // terrain along diagonal river runs differs. See `world_core::rivers`.
            // v8: the carved bank wall is capped to a bounded height above the
            // water (`MAX_BRINK`) so steep valleys read as a house-scale step
            // rather than a tall cliff; this changes terrain height/slope on steep
            // riverbanks, and thus the flora seated there. See `world_core::rivers`.
            // v9: low riverbanks are raised into a minimum levee (`BRINK_MIN`) and
            // land flora additionally rejects ground below the water sheet, so
            // bank terrain and the plants seated near channels both differ. See
            // `world_core::rivers` and `world_core::content::flora`.
            // v10: the noise core swapped from the `noise` crate's classic
            // OpenSimplex to the custom branchless 4D simplex in
            // `world_core::noise4` (~2.2× faster, character-calibrated). Every
            // octave is a different field now, so the whole baked world differs
            // and old snapshots must be rejected.
            version: 10,
            seed: config.world.seed,
            sea_level: config.sea_level,
            biome: &config.biome,
            heightmap: &config.heightmap,
            rivers: &config.rivers,
            flora: herbarium
                .plants
                .iter()
                .map(|plant| BaseGenerationPlant {
                    name: &plant.name,
                    kind: &plant.species.body_plan.kind,
                    height_range: plant.species.body_plan.max_height,
                    placement: &plant.placement,
                })
                .collect(),
        }
    }
}

/// A deduplicated, indexed view of the herbarium for world generation.
/// Each species appears once; `species_index` values in `PlantInstance` refer
/// to indices in `species`.
#[derive(Clone)]
pub struct PlantRegistry {
    pub species: Vec<PlantSpeciesInfo>,
}

/// Flattened info for one species used during placement and rendering.
#[derive(Clone)]
pub struct PlantSpeciesInfo {
    pub name: String,
    pub kind: String,
    pub crown_shape: String,
    pub crown_base: f32,
    pub aspect_ratio: f32,
    pub trunk_thickness_ratio: f32,
    pub trunk_taper: f32,
    pub bark_color: [f32; 3],
    pub leaf_color: [f32; 3],
    pub height_range: [f32; 2],
    pub placement: PlacementConfig,
    /// Full species config for procedural mesh generation.
    pub species_config: SpeciesConfig,
}

impl PlantRegistry {
    pub fn from_herbarium(herb: &Herbarium) -> Self {
        let mut species = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for entry in &herb.plants {
            if !seen_names.insert(entry.name.clone()) {
                log::warn!("skipping duplicate herbarium entry: {}", entry.name);
                continue;
            }

            let bark = crate::world_core::color::hsl_to_linear(
                entry.species.color.bark.h,
                entry.species.color.bark.s,
                entry.species.color.bark.l,
            );
            let leaf = crate::world_core::color::hsl_to_linear(
                entry.species.color.leaf.h,
                entry.species.color.leaf.s,
                entry.species.color.leaf.l,
            );

            species.push(PlantSpeciesInfo {
                name: entry.name.clone(),
                kind: entry.species.body_plan.kind.clone(),
                crown_shape: entry.species.crown.shape.clone(),
                crown_base: entry.species.crown.crown_base,
                aspect_ratio: entry.species.crown.aspect_ratio,
                trunk_thickness_ratio: entry.species.trunk.thickness_ratio,
                trunk_taper: entry.species.trunk.taper,
                bark_color: bark,
                leaf_color: leaf,
                height_range: entry.species.body_plan.max_height,
                placement: entry.placement.clone(),
                species_config: entry.species.clone(),
            });
        }

        Self { species }
    }
}

const SPECIES_PRESETS: &[(&str, &str)] = &[
    ("Oak", include_str!("plant_gen/species/oak.json")),
    ("Birch", include_str!("plant_gen/species/birch.json")),
    ("Acacia", include_str!("plant_gen/species/acacia.json")),
    ("Palm", include_str!("plant_gen/species/palm.json")),
    ("Shrub", include_str!("plant_gen/species/shrub.json")),
    ("Spruce", include_str!("plant_gen/species/spruce.json")),
    ("Willow", include_str!("plant_gen/species/willow.json")),
    ("Cattail", include_str!("plant_gen/species/cattail.json")),
];

fn default_placement(name: &str) -> PlacementConfig {
    match name {
        "Oak" => PlacementConfig {
            biomes: vec!["Forest".into(), "Grassland".into()],
            weight: 1.0,
            min_moisture: 0.3,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 120.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
            spread_radius: 25.0,
            spread_chance: 0.3,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: TREE_LIFESPAN_HOURS,
            snag_hours: TREE_SNAG_HOURS,
            aquatic: false,
        },
        "Birch" => PlacementConfig {
            biomes: vec!["Forest".into()],
            weight: 0.7,
            min_moisture: 0.5,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 120.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
            spread_radius: 25.0,
            spread_chance: 0.3,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: TREE_LIFESPAN_HOURS,
            snag_hours: TREE_SNAG_HOURS,
            aquatic: false,
        },
        "Acacia" => PlacementConfig {
            biomes: vec!["Desert".into(), "Grassland".into()],
            weight: 0.6,
            min_moisture: 0.0,
            max_moisture: 0.5,
            min_altitude: 0.0,
            max_altitude: 100.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
            spread_radius: 25.0,
            spread_chance: 0.3,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: TREE_LIFESPAN_HOURS,
            snag_hours: TREE_SNAG_HOURS,
            aquatic: false,
        },
        "Palm" => PlacementConfig {
            biomes: vec!["Grassland".into(), "Forest".into()],
            weight: 0.5,
            min_moisture: 0.4,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 60.0,
            near_water_boost: 0.5,
            max_slope: 1.0,
            spread_radius: 25.0,
            spread_chance: 0.3,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: TREE_LIFESPAN_HOURS,
            snag_hours: TREE_SNAG_HOURS,
            aquatic: false,
        },
        "Shrub" => PlacementConfig {
            biomes: vec!["Forest".into(), "Grassland".into()],
            weight: 1.0,
            min_moisture: 0.2,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 120.0,
            near_water_boost: 0.0,
            max_slope: 0.8,
            spread_radius: 25.0,
            spread_chance: 0.3,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: SHRUB_LIFESPAN_HOURS,
            snag_hours: SHRUB_SNAG_HOURS,
            aquatic: false,
        },
        "Spruce" => PlacementConfig {
            biomes: vec!["Forest".into()],
            weight: 0.8,
            min_moisture: 0.3,
            max_moisture: 1.0,
            min_altitude: 60.0,
            max_altitude: 160.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
            spread_radius: 25.0,
            spread_chance: 0.3,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: TREE_LIFESPAN_HOURS,
            snag_hours: TREE_SNAG_HOURS,
            aquatic: false,
        },
        "Willow" => PlacementConfig {
            biomes: vec!["Forest".into(), "Grassland".into()],
            weight: 0.4,
            min_moisture: 0.5,
            max_moisture: 1.0,
            min_altitude: 0.0,
            max_altitude: 80.0,
            near_water_boost: 0.7,
            max_slope: 1.0,
            spread_radius: 25.0,
            spread_chance: 0.3,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: TREE_LIFESPAN_HOURS,
            snag_hours: TREE_SNAG_HOURS,
            aquatic: false,
        },
        "Cattail" => PlacementConfig {
            // Cattails fringe water in every biome a river or lake can reach.
            biomes: vec!["Forest".into(), "Grassland".into(), "Desert".into()],
            weight: 1.0,
            min_moisture: 0.0,
            max_moisture: 1.0,
            // Allow the shallow-water band, which dips to (and just below) sea
            // level; the aquatic guard handles the wetness range, not altitude.
            min_altitude: 0.0,
            max_altitude: 160.0,
            near_water_boost: 0.0,
            max_slope: 1.0,
            // Base-only for v1: a zero spread chance keeps reed beds out of the
            // lifecycle spread pass, so its wetness guard needs no aquatic
            // exemption. Reed beds don't migrate fast; revisit later.
            spread_radius: 0.0,
            spread_chance: 0.0,
            seedling_hours: 48.0,
            young_hours: 96.0,
            lifespan_hours: IMMORTAL,
            snag_hours: 0.0,
            aquatic: true,
        },
        _ => PlacementConfig::default(),
    }
}

impl Herbarium {
    /// Create a new herbarium entry with Oak defaults and the given name.
    pub fn new_entry(name: String) -> HerbariumEntry {
        let mut species: SpeciesConfig =
            serde_json::from_str(SPECIES_PRESETS[0].1).expect("invalid oak.json");
        species.name = name.clone();
        HerbariumEntry {
            name,
            species,
            placement: PlacementConfig::default(),
        }
    }

    pub fn default_seeded() -> Self {
        let plants = SPECIES_PRESETS
            .iter()
            .map(|(name, json)| {
                let species: SpeciesConfig = serde_json::from_str(json)
                    .unwrap_or_else(|e| panic!("invalid {name}.json: {e}"));
                HerbariumEntry {
                    name: name.to_string(),
                    placement: default_placement(name),
                    species,
                }
            })
            .collect();
        Self { plants }
    }
}

impl Herbarium {
    pub fn load(storage: &dyn Storage) -> Self {
        match storage.load("herbarium") {
            Some(contents) => match serde_json::from_str(&contents) {
                Ok(herb) => {
                    log::info!("loaded herbarium");
                    herb
                }
                Err(e) => {
                    log::warn!("failed to parse herbarium: {e}");
                    Self::default_seeded()
                }
            },
            None => {
                let herb = Self::default_seeded();
                if let Err(e) = herb.save(storage) {
                    log::warn!("failed to write initial herbarium: {e}");
                }
                herb
            }
        }
    }

    pub fn save(&self, storage: &dyn Storage) -> anyhow::Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        storage.save("herbarium", &json)?;
        log::info!("saved herbarium");
        Ok(())
    }
}
