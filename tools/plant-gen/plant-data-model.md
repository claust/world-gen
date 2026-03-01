# Procedural Plant Data Model

Reference document for the parametric plant species system. Every plant — from spruce to shrub to palm — is defined by a single `PlantSpecies` JSON file that drives both generation and rendering.

## JSON Schema

```jsonc
{
  "name": "Species Name",          // unique identifier, also seeds the RNG

  "body_plan": {
    "kind": "tree",                // tree | shrub | grass | succulent | fern
    "stem_count": 1,               // 1 = single trunk, 3-12 = multi-stem shrub
    "max_height": [12, 18]         // [min, max] mature height in meters
  },

  "trunk": {
    "taper": 0.4,                  // 0.0 = cylinder, 0.9 = extreme cone
    "base_flare": 0.3,             // root flare / buttressing, 0.0-1.0
    "straightness": 0.85,          // 1.0 = perfectly straight, 0.0 = bent/curved
    "thickness_ratio": 0.05        // trunk radius / tree height (0.02 birch, 0.06 baobab)
  },

  "branching": {
    "apical_dominance": 0.3,       // 0.0 = bushy, 0.5 = spreading, 1.0 = strict central leader
    "max_depth": 3,                // recursion levels (0 = no branches, 1-4 = detail)
    "arrangement": {               // how branches attach to parent
      "type": "spiral",            // spiral | whorled | opposite | random
      "angle": 137.5               // spiral: golden angle; whorled: branches per whorl
    },
    "branches_per_node": [2, 4],   // [min, max] branches at each attachment point
    "insertion_angle": {           // angle from parent axis (degrees), varies along trunk
      "base": [65, 80],            // [min, max] at trunk base (near ground)
      "tip": [35, 50]              // [min, max] at trunk tip (near crown top)
    },
    "length_profile": "dome",      // conical | dome | columnar | vase | layered
    "child_length_ratio": 0.65,    // child / parent length
    "child_thickness_ratio": 0.7,  // child / parent radius
    "gravity_response": 0.5,       // 0.0 = rigid branches, 1.0 = weeping
    "randomness": 0.4              // 0.0 = perfectly symmetric, 1.0 = chaotic
  },

  "crown": {
    "shape": "dome",               // conical | columnar | dome | oval | vase | umbrella | weeping | irregular | fan_top
    "crown_base": 0.25,            // where branches start (0.0 = ground, 0.8 = tall bare trunk)
    "aspect_ratio": 1.3,           // crown width / height (<1 tall, >1 wide)
    "density": 0.7,                // foliage fill (0.2 = airy, 1.0 = solid mass)
    "asymmetry": 0.2               // 0.0 = perfect symmetry, 0.5 = natural lean
  },

  "foliage": {
    "style": "broadleaf",          // broadleaf | needle | scale_leaf | palm_frond | none
    "leaf_size": [0.02, 0.05],     // [min, max] in meters (drives blob radius in renderer)
    "cluster_strategy": {          // how leaves group
      "type": "clusters",          // individual | clusters | dense_mass | ring
      "count": 5                   // blobs per cluster (for clusters/ring types)
    },
    "droop": 0.3,                  // 0.0 = stiff upward, 1.0 = hanging down
    "coverage": 0.4                // fraction of branch length with leaves (tip-only vs full)
  },

  "color": {
    "bark":  { "h": 25,  "s": 0.40, "l": 0.25 },   // HSL, h in degrees
    "leaf":  { "h": 120, "s": 0.50, "l": 0.35 },
    "leaf_variance": 0.15          // how much individual blobs vary in hue/lightness
  }
}
```

## Botanical Principles

The data model encodes five fundamental growth principles that produce the full range of plant forms.

### 1. Apical Dominance

How strongly the main trunk tip dominates over side branches. This single parameter is the difference between a spruce and an oak.

| Value | Effect | Examples |
|-------|--------|----------|
| 0.0-0.2 | No dominant leader. Multi-stem, bushy. | Shrubs, hedges |
| 0.3-0.5 | Trunk forks into co-dominant leaders. Broad crown. | Oaks, maples, elms |
| 0.6-0.8 | Clear trunk with moderate branching. | Birch, ash, beech |
| 0.9-1.0 | Single strict trunk, small subordinate branches. | Spruces, pines, poplars |

### 2. Branch Geometry

Three sub-properties define how branches relate to their parent:

- **Insertion angle**: Acute (15-30deg) = upright columnar. Wide (60-90deg) = spreading horizontal. Varies along the trunk via the gradient range.
- **Length profile**: Controls how branch length varies with height. Conical = longest at bottom (Christmas tree). Dome = longest at mid-height (oak). Vase = longest at top (elm).
- **Arrangement**: Spiral (golden angle, most broadleaf), Whorled (rings of branches, conifers), Opposite (pairs, maples), Random (shrubs).

### 3. Gravity Response

Branches droop under their own weight. The amount depends on `gravity_response`:

| Value | Effect | Examples |
|-------|--------|----------|
| 0.0-0.2 | Rigid, branches hold position | Acacias, young conifers |
| 0.3-0.5 | Moderate natural droop | Oaks, maples |
| 0.6-0.8 | Noticeable drooping tips | Birch, cedar |
| 0.9-1.0 | Extreme weeping / cascading | Willows |

### 4. Self-Similarity (Recursive Branching)

A branch looks like a smaller version of the whole tree. The `child_length_ratio` and `child_thickness_ratio` control how much each level shrinks. Leonardo's rule: cross-sectional area is roughly preserved at branching points (child_thickness_ratio ~0.7 = 1/sqrt(2)).

### 5. Crown Envelope

The crown shape acts as a bounding volume that constrains where branches grow. Branches inside the envelope keep growing; branches that would exit are terminated.

**Crown Shapes:**

| Shape | Silhouette | Typical species |
|-------|-----------|-----------------|
| `conical` | Christmas tree triangle | Spruce, fir, young pine |
| `columnar` | Tall narrow cylinder | Poplar, cypress, Lombardy |
| `dome` | Hemisphere | Mature oak, maple |
| `oval` | Egg shape, taller than wide | Beech, linden |
| `vase` | Narrow base, widening up | Elm |
| `umbrella` | Flat-topped parasol | Acacia, old Scots pine |
| `weeping` | Dome cascading below equator | Willow |
| `irregular` | Asymmetric blob | Ancient/gnarly trees |
| `fan_top` | Bare trunk + rosette at top | Palms, tree ferns |

**Length Profiles:**

| Profile | Branch length distribution | Typical species |
|---------|--------------------------|-----------------|
| `conical` | Longest at bottom, decreasing up | Spruces, young pines |
| `dome` | Longest at mid-height | Oaks, maples, beeches |
| `columnar` | Roughly equal throughout | Poplars, cypresses |
| `vase` | Shortest at bottom, longest near top | Elms |
| `layered` | Alternating long/short tiers | Mature pines, Norfolk pine |

## Axes of Visual Impact

Ordered by how much visual difference they create (most impactful first):

1. **Crown silhouette** (shape + aspect_ratio) — readable at any distance
2. **Apical dominance** — trunk vs branches character
3. **Gravity response** — drooping vs rigid
4. **Foliage style** (broadleaf/needle/palm/none) — surface texture
5. **Crown base height** — bare trunk vs branches-to-ground
6. **Density** — solid mass vs airy skeleton
7. **Branch arrangement** (spiral/whorled/opposite) — subtle rhythm
8. **Color palette** — species identity at medium range

For LOD: drop detail from the bottom up. At distance, only silhouette and color matter.

## Rendering Pipeline

### Artifacts

The plant-gen tool produces two output formats from the same species JSON:

- **SVG** (`.svg`): 2D side-view sketch for rapid parameter iteration. Instant feedback, viewable in any browser. Good for tuning crown silhouette, branch structure, and color. Cannot show 3D depth or how the tree looks from other angles.
- **GLB** (`.glb`): Full 3D mesh viewable in Blender, VS Code (glTF extension), online glTF viewers, or any tool that supports glTF 2.0. Branches are tapered 8-sided cylinders, foliage is low-poly icosahedra. Vertex colors encode bark/leaf materials.

### Design-Time Workflow

```
Species JSON ──→ render.ts ──→ SVG  (2D sketch, instant iteration)
                            └──→ GLB  (3D mesh, preview in any viewer)
```

Edit JSON parameters → run the tool → inspect output → repeat. The SVG is fastest for silhouette tuning. Switch to GLB when you need to verify 3D structure (branch depth distribution, how the spiral arrangement looks from above, etc.).

### Runtime Pipeline (future, in-engine)

```
Species JSON + per-tree seed ──→ Rust mesh generator ──→ vertex/index buffers ──→ GPU upload
```

The Rust mesh generator will implement the same recursive branching algorithm as render.ts, producing vertex buffers directly. Trees are generated per-chunk on the CPU and uploaded to the GPU, following the same pattern as the existing terrain mesh pipeline. Each tree gets a unique seed derived from its world position, so every tree is different while remaining deterministic.

### Mesh Structure

Both the GLB output and the future runtime mesh share the same structure:

- **Branches**: Tapered cylinders (8-sided cross-section) with per-vertex bark color
- **Foliage**: Low-poly icosahedra (12 vertices, 20 faces) with per-vertex leaf color and hue/lightness variance
- **Single mesh**: All geometry merged into one mesh with vertex colors distinguishing bark from foliage — matches the existing `tree.glb` convention used by the game renderer's `InstancedPass`

### Why Not Just SVG?

SVG is fundamentally 2D. It projects the tree onto a flat plane, losing the z-axis entirely. You cannot see:
- How branches distribute in depth (spiral vs whorled looks the same in 2D)
- What the tree looks like from above, at 3/4 angle, or in the game camera
- How foliage clusters overlap in 3D space

The SVG remains valuable as a fast sketch tool. The GLB provides the ground truth 3D preview.

## Rendering Tool

```bash
bun tools/plant-gen/render.ts <species.json> [output.svg|.glb]
bun tools/plant-gen/render.ts examples/oak.json                    # → oak.svg (default)
bun tools/plant-gen/render.ts examples/oak.json oak.glb             # → oak.glb
bun tools/plant-gen/render.ts examples/oak.json --format glb        # → oak.glb
```

Reads a species JSON and generates either SVG or GLB based on output extension or `--format` flag. Default is SVG.

Example species files are in `tools/plant-gen/examples/`.
