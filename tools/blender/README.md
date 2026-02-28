# Blender Asset Pipeline

Procedural 3D model generation scripts for the world-gen renderer. Each script builds a model in Blender, renders preview images for visual iteration, and exports a GLB file that the game loads at runtime.

## Quick Start

```bash
# Preview only (renders to tools/blender/renders/)
/Applications/Blender.app/Contents/MacOS/Blender --background --python tools/blender/tree.py

# Preview + export GLB to assets/models/
/Applications/Blender.app/Contents/MacOS/Blender --background --python tools/blender/tree.py -- --export
```

## Naming Convention

Script and asset names must match. The game's model loader looks for `assets/models/{name}.glb`, so:

| Script | Output | Loaded via |
|---|---|---|
| `tools/blender/tree.py` | `assets/models/tree.glb` | `try_load_model(device, "tree")` |
| `tools/blender/house.py` | `assets/models/house.glb` | `try_load_model(device, "house")` |

The `.py` source is the primary asset; the `.glb` is a generated artifact.

## Script Structure

Every script should follow this pattern:

```
1. Config constants at the top (dimensions, colors, subdivisions)
2. Build the model geometry
3. Bake vertex colors
4. Render preview images (always)
5. Export GLB (only with --export flag)
6. Print summary
```

See `tree.py` as the reference implementation.

## Critical Requirements for the Game Renderer

The model loader (`src/renderer_wgpu/model_loader.rs`) has specific expectations. Models that don't follow these rules will render incorrectly or not at all.

### Use vertex colors, not materials

The renderer reads the `COLOR_0` vertex attribute. It ignores Principled BSDF material colors entirely. Missing vertex colors fall back to white.

Bake colors onto mesh data before export:

```python
def bake_vertex_colors(obj, rgba):
    mesh = obj.data
    if not mesh.color_attributes:
        mesh.color_attributes.new(name="Color", type='FLOAT_COLOR', domain='CORNER')
    attr = mesh.color_attributes[0]
    for i in range(len(attr.data)):
        attr.data[i].color = rgba
```

You still need materials with a `ShaderNodeVertexColor` node connected to the BSDF `Base Color` input -- otherwise the glTF exporter silently drops the vertex color data.

```python
color_node = tree.nodes.new(type='ShaderNodeVertexColor')
color_node.layer_name = "Color"
tree.links.new(color_node.outputs["Color"], bsdf.inputs["Base Color"])
```

### Single mesh, single primitive

The loader reads only the **first primitive of the first mesh**:

```rust
let mesh = gltf.document.meshes().next();       // first mesh only
let primitive = mesh.primitives().next();        // first primitive only
```

Before export, all parts must be:

1. **Joined** into one mesh object (`bpy.ops.object.join()`)
2. **Collapsed to one material** (multiple materials = multiple primitives)

```python
# Join all mesh parts
bpy.ops.object.join()

# Collapse to single material
while len(obj.data.materials) > 1:
    obj.data.materials.pop(index=1)
```

### Origin at ground level

The game places models with their origin at the terrain surface. The model's origin must be at the base (ground contact point), not at the center.

In Blender (Z-up), set origin to Z=0. This becomes Y=0 in glTF (Y-up):

```python
bpy.context.scene.cursor.location = (0, 0, 0)
bpy.ops.object.origin_set(type='ORIGIN_CURSOR')
```

### GLB export settings

```python
bpy.ops.export_scene.gltf(
    filepath=output_path,
    export_format='GLB',
    export_normals=True,
    export_materials='EXPORT',
    export_texcoords=True,
    export_yup=True,            # Blender Z-up -> glTF Y-up
    export_apply=True,
    use_selection=False,
)
```

Note: `export_colors` was removed in Blender 5.0 -- don't include it.

## Preview Rendering

Scripts render preview images to `tools/blender/renders/` for visual iteration before exporting. This lets you tweak the model without rebuilding the game.

Add a ground plane, sun + fill light, and sky background for context. Remove these render-only objects before GLB export.

Render settings: Cycles CPU, 64 samples, 1024x1024 PNG.

## Development Workflow

```
1. Edit tools/blender/{name}.py
2. Run Blender (no --export) to get preview renders
3. View tools/blender/renders/{name}_*.png
4. Iterate on the model until happy
5. Run with --export to produce assets/models/{name}.glb
6. cargo run --release to see it in-game
7. Use the debug API screenshot to verify:
   curl -X POST http://127.0.0.1:7777/api/command \
     -H 'Content-Type: application/json' \
     -d '{"id":"ss","type":"take_screenshot"}'
8. Check captures/latest.png
```

## Blender Version

Scripts target **Blender 5.0** Python API. Run headlessly via:

```
/Applications/Blender.app/Contents/MacOS/Blender --background --python <script.py> -- [flags]
```

Note the `--` separator before script-specific flags like `--export`.
