"""
Blender Python script: Generate a stylized low-poly tree and render preview images.

Run headlessly:
  /Applications/Blender.app/Contents/MacOS/Blender --background --python tools/blender/tree.py

Outputs:
  - tools/blender/renders/tree_front.png   (preview render)
  - tools/blender/renders/tree_side.png    (preview render)
  - assets/models/tree.glb                 (export, only when --export flag is passed)
"""

import bpy
import os
import sys
import math

# ---------------------------------------------------------------------------
# Config — Tweak these values to adjust the tree
# ---------------------------------------------------------------------------

# Trunk — tapered cone shape
TRUNK_RADIUS_BASE = 0.40      # meters (wider at ground)
TRUNK_RADIUS_TOP = 0.18       # meters (narrower at crown)
TRUNK_HEIGHT = 7.0             # meters
TRUNK_SIDES = 8                # low-poly
TRUNK_COLOR = (0.35, 0.22, 0.12, 1.0)   # warm brown

# Canopy — main blob + satellite blobs for organic silhouette
CANOPY_MAIN_RADIUS = 3.0      # meters
CANOPY_SUBDIVISIONS = 2        # icosphere detail (low-poly)
CANOPY_COLOR = (0.16, 0.40, 0.18, 1.0)  # forest green
CANOPY_SQUASH_Z = 0.70         # vertical flatten factor

# Extra canopy blobs: (x, y, z_offset_from_main_center, radius, squash_z)
CANOPY_BLOBS = [
    ( 1.6,  0.6, -0.6,  2.0, 0.65),
    (-1.4,  1.2, -0.3,  2.2, 0.70),
    ( 0.4, -1.7,  0.3,  1.9, 0.60),
    (-0.9, -1.0,  0.9,  1.6, 0.75),
]

ROUGHNESS = 0.9  # matte, stylized look

# Paths
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.abspath(os.path.join(SCRIPT_DIR, "..", ".."))
RENDER_DIR = os.path.join(SCRIPT_DIR, "renders")
EXPORT_FILE = os.path.join(PROJECT_ROOT, "assets", "models", "tree.glb")

# Render settings
RENDER_RES = 1024
RENDER_SAMPLES = 64


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def make_material(name, rgba, roughness=ROUGHNESS):
    mat = bpy.data.materials.new(name=name)
    mat.use_nodes = True
    tree = mat.node_tree
    bsdf = tree.nodes.get("Principled BSDF")
    if bsdf:
        bsdf.inputs["Base Color"].default_value = rgba
        bsdf.inputs["Roughness"].default_value = roughness

        # Connect a Color Attribute node so the glTF exporter includes
        # vertex colors in the GLB (it skips them if unused in the node tree).
        color_node = tree.nodes.new(type='ShaderNodeVertexColor')
        color_node.layer_name = "Color"
        tree.links.new(color_node.outputs["Color"], bsdf.inputs["Base Color"])
    return mat


def build_trunk(trunk_mat):
    """Create a tapered trunk (cone) with base at Z=0."""
    bpy.ops.mesh.primitive_cone_add(
        vertices=TRUNK_SIDES,
        radius1=TRUNK_RADIUS_BASE,
        radius2=TRUNK_RADIUS_TOP,
        depth=TRUNK_HEIGHT,
        end_fill_type='NGON',
        calc_uvs=True,
        location=(0, 0, TRUNK_HEIGHT / 2),
    )
    trunk = bpy.context.active_object
    trunk.name = "Trunk"
    trunk.data.materials.append(trunk_mat)
    bake_vertex_colors(trunk, TRUNK_COLOR)
    bpy.ops.object.shade_smooth()
    return trunk


def build_canopy(canopy_mat):
    """Create a multi-blob canopy: one main sphere + satellite blobs, joined."""
    canopy_center_z = TRUNK_HEIGHT + CANOPY_MAIN_RADIUS * CANOPY_SQUASH_Z * 0.35

    # Main canopy sphere
    bpy.ops.mesh.primitive_ico_sphere_add(
        subdivisions=CANOPY_SUBDIVISIONS,
        radius=CANOPY_MAIN_RADIUS,
        calc_uvs=True,
        location=(0, 0, canopy_center_z),
    )
    main = bpy.context.active_object
    main.name = "Canopy_Main"
    main.data.materials.append(canopy_mat)
    bake_vertex_colors(main, CANOPY_COLOR)
    main.scale.z = CANOPY_SQUASH_Z
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    bpy.ops.object.shade_flat()

    parts = [main]

    # Satellite blobs
    for i, (dx, dy, dz, r, sz) in enumerate(CANOPY_BLOBS):
        bpy.ops.mesh.primitive_ico_sphere_add(
            subdivisions=CANOPY_SUBDIVISIONS,
            radius=r,
            calc_uvs=True,
            location=(dx, dy, canopy_center_z + dz),
        )
        blob = bpy.context.active_object
        blob.name = f"Canopy_Blob_{i}"
        blob.data.materials.append(canopy_mat)
        bake_vertex_colors(blob, CANOPY_COLOR)
        blob.scale.z = sz
        bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
        bpy.ops.object.shade_flat()
        parts.append(blob)

    # Join into single mesh
    bpy.ops.object.select_all(action='DESELECT')
    for p in parts:
        p.select_set(True)
    bpy.context.view_layer.objects.active = main
    bpy.ops.object.join()

    canopy = bpy.context.active_object
    canopy.name = "Canopy"
    return canopy


def bake_vertex_colors(obj, rgba):
    """Add a COLOR_0 vertex color layer and fill it with a solid color.

    The game renderer reads per-vertex colors (not materials), so we
    need to bake the color into the mesh data for the GLB export.
    """
    mesh = obj.data
    if not mesh.color_attributes:
        mesh.color_attributes.new(name="Color", type='FLOAT_COLOR', domain='CORNER')
    attr = mesh.color_attributes[0]
    for i in range(len(attr.data)):
        attr.data[i].color = rgba


def parent_objects(child, parent):
    bpy.ops.object.select_all(action='DESELECT')
    child.select_set(True)
    parent.select_set(True)
    bpy.context.view_layer.objects.active = parent
    bpy.ops.object.parent_set(type='OBJECT', keep_transform=True)


# ---------------------------------------------------------------------------
# Render setup
# ---------------------------------------------------------------------------

def add_ground():
    bpy.ops.mesh.primitive_plane_add(size=30, location=(0, 0, 0))
    ground = bpy.context.active_object
    ground.name = "Ground"
    ground.data.materials.append(
        make_material("Ground", (0.30, 0.38, 0.20, 1.0), roughness=0.95)
    )
    return ground


def setup_lighting():
    # Sun — main key light
    sun = bpy.data.lights.new("Sun", type='SUN')
    sun.energy = 3.0
    sun.color = (1.0, 0.95, 0.85)
    sun_obj = bpy.data.objects.new("Sun", sun)
    sun_obj.rotation_euler = (math.radians(45), math.radians(10), math.radians(30))
    bpy.context.collection.objects.link(sun_obj)

    # Fill — soft area light from opposite side
    fill = bpy.data.lights.new("Fill", type='AREA')
    fill.energy = 50.0
    fill.size = 8.0
    fill.color = (0.7, 0.85, 1.0)
    fill_obj = bpy.data.objects.new("Fill", fill)
    fill_obj.location = (-8, 6, 5)
    bpy.context.collection.objects.link(fill_obj)

    # Point fill at tree mid-height
    target = bpy.data.objects.new("FillTarget", None)
    target.location = (0, 0, TRUNK_HEIGHT * 0.5)
    bpy.context.collection.objects.link(target)
    c = fill_obj.constraints.new(type='TRACK_TO')
    c.target = target
    c.track_axis = 'TRACK_NEGATIVE_Z'
    c.up_axis = 'UP_Y'

    # Sky background
    world = bpy.data.worlds.new("World")
    world.use_nodes = True
    bg = world.node_tree.nodes.get("Background")
    if bg:
        bg.inputs["Color"].default_value = (0.55, 0.70, 0.90, 1.0)
        bg.inputs["Strength"].default_value = 0.5
    bpy.context.scene.world = world


def make_camera(location, name="Camera"):
    cam = bpy.data.cameras.new(name)
    cam.lens = 50
    obj = bpy.data.objects.new(name, cam)
    obj.location = location
    bpy.context.collection.objects.link(obj)

    # Track to tree center
    target = bpy.data.objects.new(f"{name}Target", None)
    target.location = (0, 0, TRUNK_HEIGHT * 0.55)
    bpy.context.collection.objects.link(target)
    c = obj.constraints.new(type='TRACK_TO')
    c.target = target
    c.track_axis = 'TRACK_NEGATIVE_Z'
    c.up_axis = 'UP_Y'

    return obj


def render_to(camera_obj, filepath):
    scene = bpy.context.scene
    scene.camera = camera_obj
    scene.render.engine = 'CYCLES'
    scene.cycles.device = 'CPU'
    scene.cycles.samples = RENDER_SAMPLES
    scene.render.resolution_x = RENDER_RES
    scene.render.resolution_y = RENDER_RES
    scene.render.image_settings.file_format = 'PNG'
    scene.render.filepath = filepath
    bpy.ops.render.render(write_still=True)
    print(f"  Rendered: {filepath}")


# ---------------------------------------------------------------------------
# Export
# ---------------------------------------------------------------------------

def export_glb(render_objects):
    """Remove render-only objects, join tree meshes, then export as GLB."""
    # Remove render-only objects (ground, lights, cameras, empties)
    for obj in render_objects:
        bpy.data.objects.remove(obj, do_unlink=True)
    for obj in list(bpy.data.objects):
        if obj.type in ('CAMERA', 'LIGHT', 'EMPTY'):
            bpy.data.objects.remove(obj, do_unlink=True)

    # Join all remaining meshes (trunk + canopy) into a single object.
    # The model loader only reads the first mesh, so we must merge them.
    # Vertex colors are preserved per-vertex through the join.
    bpy.ops.object.select_all(action='DESELECT')
    meshes = [o for o in bpy.data.objects if o.type == 'MESH']
    for m in meshes:
        m.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]
    bpy.ops.object.parent_clear(type='CLEAR_KEEP_TRANSFORM')
    bpy.ops.object.join()
    tree = bpy.context.active_object
    tree.name = "Tree"

    # Collapse to a single material so the glTF exporter produces one
    # primitive.  The game renderer uses vertex colors, not materials.
    while len(tree.data.materials) > 1:
        tree.data.materials.pop(index=1)
    tree.data.materials[0].name = "Tree"

    # Reset origin to world origin (0,0,0) so the trunk base sits at
    # ground level in the game (Y=0 in glTF Y-up space).
    bpy.context.scene.cursor.location = (0, 0, 0)
    bpy.ops.object.origin_set(type='ORIGIN_CURSOR')

    os.makedirs(os.path.dirname(EXPORT_FILE), exist_ok=True)
    bpy.ops.export_scene.gltf(
        filepath=EXPORT_FILE,
        export_format='GLB',
        export_normals=True,
        export_materials='EXPORT',
        export_texcoords=True,
        export_yup=True,
        export_apply=True,
        use_selection=False,
    )
    print(f"  Exported: {EXPORT_FILE}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    do_export = "--export" in sys.argv

    # Clean scene
    bpy.ops.wm.read_factory_settings(use_empty=True)

    # Build tree
    trunk_mat = make_material("Trunk", TRUNK_COLOR)
    canopy_mat = make_material("Canopy", CANOPY_COLOR)

    trunk = build_trunk(trunk_mat)
    canopy = build_canopy(canopy_mat)
    parent_objects(canopy, trunk)

    # Render setup
    ground = add_ground()
    setup_lighting()

    os.makedirs(RENDER_DIR, exist_ok=True)

    cam_front = make_camera((11, -9, 7), "CamFront")
    render_to(cam_front, os.path.join(RENDER_DIR, "tree_front.png"))

    cam_side = make_camera((-4, -13, 5), "CamSide")
    render_to(cam_side, os.path.join(RENDER_DIR, "tree_side.png"))

    # Export if requested
    if do_export:
        export_glb([ground])

    # Summary
    print(f"\n=== Tree Generation Complete ===")
    print(f"  Trunk: tapered {TRUNK_SIDES}-sided, base_r={TRUNK_RADIUS_BASE}m, top_r={TRUNK_RADIUS_TOP}m, h={TRUNK_HEIGHT}m")
    print(f"  Canopy: {1 + len(CANOPY_BLOBS)} blobs, main_r={CANOPY_MAIN_RADIUS}m, squash={CANOPY_SQUASH_Z}")
    print(f"  Renders: {RENDER_DIR}/")
    if do_export:
        print(f"  GLB: {EXPORT_FILE}")


if __name__ == "__main__":
    main()
