"""
Blender Python script: Generate a stylized low-poly house and render preview images.

Run headlessly:
  /Applications/Blender.app/Contents/MacOS/Blender --background --python tools/blender/house.py

Outputs:
  - tools/blender/renders/house_front.png  (preview render)
  - tools/blender/renders/house_side.png   (preview render)
  - assets/models/house.glb               (export, only when --export flag is passed)
"""

import bpy
import bmesh
import os
import sys
import math

# ---------------------------------------------------------------------------
# Config — Tweak these values to adjust the house
# ---------------------------------------------------------------------------

# Footprint
HOUSE_WIDTH = 5.0          # meters (X axis)
HOUSE_DEPTH = 4.0          # meters (Y axis in Blender, Z in game)
WALL_HEIGHT = 3.0          # meters
ROOF_PEAK = 2.0            # meters above wall top (total height = 5.0m)

# Colors (RGBA)
WALL_COLOR = (0.72, 0.63, 0.46, 1.0)    # sandy tan
ROOF_COLOR = (0.55, 0.22, 0.15, 1.0)    # terracotta red
DOOR_COLOR = (0.30, 0.18, 0.10, 1.0)    # dark brown
WINDOW_COLOR = (0.50, 0.65, 0.80, 1.0)  # pale blue

# Door dimensions
DOOR_WIDTH = 0.9
DOOR_HEIGHT = 2.2
DOOR_INSET = 0.05     # slight recess into wall

# Window dimensions
WINDOW_WIDTH = 0.8
WINDOW_HEIGHT = 0.8
WINDOW_SILL = 1.5      # height of bottom edge from ground
WINDOW_INSET = 0.05

ROUGHNESS = 0.9  # matte, stylized look

# Paths
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.abspath(os.path.join(SCRIPT_DIR, "..", ".."))
RENDER_DIR = os.path.join(SCRIPT_DIR, "renders")
EXPORT_FILE = os.path.join(PROJECT_ROOT, "assets", "models", "house.glb")

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

        # Connect vertex color node so glTF exporter includes COLOR_0
        color_node = tree.nodes.new(type='ShaderNodeVertexColor')
        color_node.layer_name = "Color"
        tree.links.new(color_node.outputs["Color"], bsdf.inputs["Base Color"])
    return mat


def bake_vertex_colors(obj, rgba):
    """Bake a solid color into the mesh's vertex color layer."""
    mesh = obj.data
    if not mesh.color_attributes:
        mesh.color_attributes.new(name="Color", type='FLOAT_COLOR', domain='CORNER')
    attr = mesh.color_attributes[0]
    for i in range(len(attr.data)):
        attr.data[i].color = rgba


def make_box(name, location, dimensions, color_rgba, mat):
    """Create a box mesh at the given location with given (width, depth, height)."""
    w, d, h = dimensions
    bpy.ops.mesh.primitive_cube_add(size=1, location=location)
    obj = bpy.context.active_object
    obj.name = name
    obj.scale = (w, d, h)
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    obj.data.materials.append(mat)
    bake_vertex_colors(obj, color_rgba)
    bpy.ops.object.shade_flat()
    return obj


# ---------------------------------------------------------------------------
# House construction
# ---------------------------------------------------------------------------

def build_walls(mat):
    """Create the main wall box with base at Z=0."""
    obj = make_box(
        "Walls",
        location=(0, 0, WALL_HEIGHT / 2),
        dimensions=(HOUSE_WIDTH, HOUSE_DEPTH, WALL_HEIGHT),
        color_rgba=WALL_COLOR,
        mat=mat,
    )
    return obj


def build_roof(mat):
    """Create a gable roof as a prism (triangular cross-section)."""
    hw = HOUSE_WIDTH / 2
    hd = HOUSE_DEPTH / 2
    top_y = WALL_HEIGHT + ROOF_PEAK

    verts = [
        # Bottom rectangle (at wall top height)
        (-hw, -hd, WALL_HEIGHT),  # 0: back-left
        ( hw, -hd, WALL_HEIGHT),  # 1: back-right
        ( hw,  hd, WALL_HEIGHT),  # 2: front-right
        (-hw,  hd, WALL_HEIGHT),  # 3: front-left
        # Ridge (top)
        (-hw, 0, top_y),          # 4: ridge-left
        ( hw, 0, top_y),          # 5: ridge-right
    ]

    # Faces: front slope, back slope, left gable, right gable
    # Winding order must produce outward-facing normals (right-hand rule)
    faces = [
        (4, 5, 2, 3),  # front slope
        (5, 4, 0, 1),  # back slope
        (4, 3, 0),      # left gable
        (5, 1, 2),      # right gable
    ]

    mesh = bpy.data.meshes.new("RoofMesh")
    mesh.from_pydata(verts, [], faces)
    mesh.update()

    obj = bpy.data.objects.new("Roof", mesh)
    bpy.context.collection.objects.link(obj)
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)

    obj.data.materials.append(mat)
    bake_vertex_colors(obj, ROOF_COLOR)
    bpy.ops.object.shade_flat()

    return obj


def build_door(mat):
    """Create a door on the front face (+Y side)."""
    obj = make_box(
        "Door",
        location=(0, HOUSE_DEPTH / 2 + DOOR_INSET, DOOR_HEIGHT / 2),
        dimensions=(DOOR_WIDTH, 0.1, DOOR_HEIGHT),
        color_rgba=DOOR_COLOR,
        mat=mat,
    )
    return obj


def build_window(name, x_pos, y_face, mat):
    """Create a window on a wall face."""
    obj = make_box(
        name,
        location=(x_pos, y_face, WINDOW_SILL + WINDOW_HEIGHT / 2),
        dimensions=(WINDOW_WIDTH, 0.1, WINDOW_HEIGHT),
        color_rgba=WINDOW_COLOR,
        mat=mat,
    )
    return obj


def build_house():
    """Assemble all house parts."""
    wall_mat = make_material("Wall", WALL_COLOR)
    roof_mat = make_material("Roof", ROOF_COLOR)
    door_mat = make_material("Door", DOOR_COLOR)
    window_mat = make_material("Window", WINDOW_COLOR)

    parts = []

    # Walls
    walls = build_walls(wall_mat)
    parts.append(walls)

    # Roof
    roof = build_roof(roof_mat)
    parts.append(roof)

    # Door on front face
    door = build_door(door_mat)
    parts.append(door)

    # Windows — one on each side wall
    side_y = HOUSE_DEPTH / 2 + WINDOW_INSET
    w_left = build_window("WindowLeft", -HOUSE_WIDTH / 2 - WINDOW_INSET, 0, window_mat)
    w_left.rotation_euler.z = math.radians(90)
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
    # Reposition after rotation
    w_left.location = (-HOUSE_WIDTH / 2 - WINDOW_INSET, 0, WINDOW_SILL + WINDOW_HEIGHT / 2)
    parts.append(w_left)

    w_right = build_window("WindowRight", HOUSE_WIDTH / 2 + WINDOW_INSET, 0, window_mat)
    w_right.rotation_euler.z = math.radians(90)
    bpy.ops.object.transform_apply(location=False, rotation=True, scale=False)
    w_right.location = (HOUSE_WIDTH / 2 + WINDOW_INSET, 0, WINDOW_SILL + WINDOW_HEIGHT / 2)
    parts.append(w_right)

    return parts


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
    # Sun
    sun = bpy.data.lights.new("Sun", type='SUN')
    sun.energy = 3.0
    sun.color = (1.0, 0.95, 0.85)
    sun_obj = bpy.data.objects.new("Sun", sun)
    sun_obj.rotation_euler = (math.radians(45), math.radians(15), math.radians(30))
    bpy.context.collection.objects.link(sun_obj)

    # Fill light
    fill = bpy.data.lights.new("Fill", type='AREA')
    fill.energy = 40.0
    fill.size = 8.0
    fill.color = (0.7, 0.85, 1.0)
    fill_obj = bpy.data.objects.new("Fill", fill)
    fill_obj.location = (-8, -6, 4)
    bpy.context.collection.objects.link(fill_obj)

    target = bpy.data.objects.new("FillTarget", None)
    target.location = (0, 0, WALL_HEIGHT * 0.5)
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
    total_h = WALL_HEIGHT + ROOF_PEAK
    cam = bpy.data.cameras.new(name)
    cam.lens = 50
    obj = bpy.data.objects.new(name, cam)
    obj.location = location
    bpy.context.collection.objects.link(obj)

    target = bpy.data.objects.new(f"{name}Target", None)
    target.location = (0, 0, total_h * 0.45)
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
    """Remove render-only objects, join house meshes, then export as GLB."""
    for obj in render_objects:
        bpy.data.objects.remove(obj, do_unlink=True)
    for obj in list(bpy.data.objects):
        if obj.type in ('CAMERA', 'LIGHT', 'EMPTY'):
            bpy.data.objects.remove(obj, do_unlink=True)

    # Join all remaining meshes into a single object
    bpy.ops.object.select_all(action='DESELECT')
    meshes = [o for o in bpy.data.objects if o.type == 'MESH']
    for m in meshes:
        m.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]
    bpy.ops.object.parent_clear(type='CLEAR_KEEP_TRANSFORM')
    bpy.ops.object.join()
    house = bpy.context.active_object
    house.name = "House"

    # Collapse to a single material (game uses vertex colors)
    while len(house.data.materials) > 1:
        house.data.materials.pop(index=1)
    house.data.materials[0].name = "House"

    # Origin at ground level
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

    # Build house
    parts = build_house()

    # Render setup
    ground = add_ground()
    setup_lighting()

    os.makedirs(RENDER_DIR, exist_ok=True)

    cam_front = make_camera((8, -7, 5), "CamFront")
    render_to(cam_front, os.path.join(RENDER_DIR, "house_front.png"))

    cam_side = make_camera((-3, -10, 4), "CamSide")
    render_to(cam_side, os.path.join(RENDER_DIR, "house_side.png"))

    # Export if requested
    if do_export:
        export_glb([ground])

    # Summary
    total_h = WALL_HEIGHT + ROOF_PEAK
    print(f"\n=== House Generation Complete ===")
    print(f"  Footprint: {HOUSE_WIDTH}m x {HOUSE_DEPTH}m, wall_h={WALL_HEIGHT}m, total_h={total_h}m")
    print(f"  Parts: walls, gable roof, door, 2 windows")
    print(f"  Renders: {RENDER_DIR}/")
    if do_export:
        print(f"  GLB: {EXPORT_FILE}")


if __name__ == "__main__":
    main()
