"""
Blender Python script: Generate a stylized low-poly fern and render preview images.

Run headlessly:
  /Applications/Blender.app/Contents/MacOS/Blender --background --python tools/blender/fern.py

With GLB export:
  /Applications/Blender.app/Contents/MacOS/Blender --background --python tools/blender/fern.py -- --export

Outputs:
  - tools/blender/renders/fern_front.png   (preview render)
  - tools/blender/renders/fern_side.png    (preview render)
  - assets/models/fern.glb                 (export, only when --export flag is passed)
"""

import bpy
import os
import sys
import math
import bmesh

# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

# Stem — short cylinder at the center
STEM_RADIUS = 0.03
STEM_HEIGHT = 0.15
STEM_SIDES = 6
STEM_COLOR = (0.15, 0.30, 0.10, 1.0)

# Fronds — elongated leaf shapes arranged radially
FROND_COUNT = 6
FROND_LENGTH = 0.35         # meters
FROND_WIDTH = 0.10          # meters at widest point
FROND_ANGLE = 35            # degrees from vertical
FROND_SEGMENTS = 3          # lengthwise subdivisions for slight curve
FROND_COLOR = (0.20, 0.45, 0.15, 1.0)
FROND_TIP_COLOR = (0.25, 0.50, 0.18, 1.0)

ROUGHNESS = 0.9

# Paths
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
PROJECT_ROOT = os.path.abspath(os.path.join(SCRIPT_DIR, "..", ".."))
RENDER_DIR = os.path.join(SCRIPT_DIR, "renders")
EXPORT_FILE = os.path.join(PROJECT_ROOT, "assets", "models", "fern.glb")

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

        color_node = tree.nodes.new(type='ShaderNodeVertexColor')
        color_node.layer_name = "Color"
        tree.links.new(color_node.outputs["Color"], bsdf.inputs["Base Color"])
    return mat


def bake_vertex_colors(obj, rgba):
    mesh = obj.data
    if not mesh.color_attributes:
        mesh.color_attributes.new(name="Color", type='FLOAT_COLOR', domain='CORNER')
    attr = mesh.color_attributes[0]
    for i in range(len(attr.data)):
        attr.data[i].color = rgba


def bake_vertex_colors_gradient(obj, base_rgba, tip_rgba):
    """Bake a gradient from base to tip based on vertex Z height."""
    mesh = obj.data
    if not mesh.color_attributes:
        mesh.color_attributes.new(name="Color", type='FLOAT_COLOR', domain='CORNER')
    attr = mesh.color_attributes[0]

    # Find Z range of vertices
    zs = [v.co.z for v in mesh.vertices]
    z_min = min(zs) if zs else 0
    z_max = max(zs) if zs else 1
    z_range = z_max - z_min if z_max > z_min else 1.0

    for poly in mesh.polygons:
        for loop_idx in poly.loop_indices:
            vert_idx = mesh.loops[loop_idx].vertex_index
            z = mesh.vertices[vert_idx].co.z
            t = (z - z_min) / z_range
            color = tuple(base_rgba[i] + t * (tip_rgba[i] - base_rgba[i]) for i in range(4))
            attr.data[loop_idx].color = color


# ---------------------------------------------------------------------------
# Geometry
# ---------------------------------------------------------------------------

def build_stem(mat):
    """Short cylinder at the base."""
    bpy.ops.mesh.primitive_cylinder_add(
        vertices=STEM_SIDES,
        radius=STEM_RADIUS,
        depth=STEM_HEIGHT,
        end_fill_type='NGON',
        calc_uvs=True,
        location=(0, 0, STEM_HEIGHT / 2),
    )
    stem = bpy.context.active_object
    stem.name = "Stem"
    stem.data.materials.append(mat)
    bake_vertex_colors(stem, STEM_COLOR)
    bpy.ops.object.shade_smooth()
    return stem


def build_frond(index, total, mat):
    """Create a single frond as a tapered flat mesh, angled outward."""
    angle_around = (2 * math.pi * index) / total

    # Create a simple tapered quad strip using bmesh
    bm = bmesh.new()

    segs = FROND_SEGMENTS
    for s in range(segs + 1):
        t = s / segs
        # Taper: wide at base, narrow at tip
        width = FROND_WIDTH * (1.0 - t * 0.85)
        length_pos = t * FROND_LENGTH

        # Slight upward curve
        curve_z = STEM_HEIGHT + length_pos * math.cos(math.radians(FROND_ANGLE))
        curve_out = length_pos * math.sin(math.radians(FROND_ANGLE))

        # Position along the frond's outward direction
        x = math.cos(angle_around) * curve_out
        y = math.sin(angle_around) * curve_out

        # Two vertices for the width (perpendicular to the outward direction)
        perp_x = -math.sin(angle_around) * width / 2
        perp_y = math.cos(angle_around) * width / 2

        bm.verts.new((x + perp_x, y + perp_y, curve_z))
        bm.verts.new((x - perp_x, y - perp_y, curve_z))

    bm.verts.ensure_lookup_table()

    # Create faces between segment pairs
    for s in range(segs):
        v0 = bm.verts[s * 2]
        v1 = bm.verts[s * 2 + 1]
        v2 = bm.verts[(s + 1) * 2 + 1]
        v3 = bm.verts[(s + 1) * 2]
        bm.faces.new((v0, v1, v2, v3))

    mesh = bpy.data.meshes.new(f"Frond_{index}")
    bm.to_mesh(mesh)
    bm.free()

    obj = bpy.data.objects.new(f"Frond_{index}", mesh)
    bpy.context.collection.objects.link(obj)
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)

    obj.data.materials.append(mat)
    bake_vertex_colors_gradient(obj, FROND_COLOR, FROND_TIP_COLOR)
    bpy.ops.object.shade_flat()

    return obj


def build_fern(stem_mat, frond_mat):
    """Build the complete fern: stem + fronds."""
    stem = build_stem(stem_mat)
    fronds = []

    for i in range(FROND_COUNT):
        frond = build_frond(i, FROND_COUNT, frond_mat)
        fronds.append(frond)

    return stem, fronds


# ---------------------------------------------------------------------------
# Render setup
# ---------------------------------------------------------------------------

def add_ground():
    bpy.ops.mesh.primitive_plane_add(size=5, location=(0, 0, 0))
    ground = bpy.context.active_object
    ground.name = "Ground"
    ground.data.materials.append(
        make_material("Ground", (0.30, 0.38, 0.20, 1.0), roughness=0.95)
    )
    return ground


def setup_lighting():
    sun = bpy.data.lights.new("Sun", type='SUN')
    sun.energy = 3.0
    sun.color = (1.0, 0.95, 0.85)
    sun_obj = bpy.data.objects.new("Sun", sun)
    sun_obj.rotation_euler = (math.radians(45), math.radians(10), math.radians(30))
    bpy.context.collection.objects.link(sun_obj)

    fill = bpy.data.lights.new("Fill", type='AREA')
    fill.energy = 30.0
    fill.size = 3.0
    fill.color = (0.7, 0.85, 1.0)
    fill_obj = bpy.data.objects.new("Fill", fill)
    fill_obj.location = (-1.5, 1.2, 1.0)
    bpy.context.collection.objects.link(fill_obj)

    target = bpy.data.objects.new("FillTarget", None)
    target.location = (0, 0, STEM_HEIGHT + FROND_LENGTH * 0.3)
    bpy.context.collection.objects.link(target)
    c = fill_obj.constraints.new(type='TRACK_TO')
    c.target = target
    c.track_axis = 'TRACK_NEGATIVE_Z'
    c.up_axis = 'UP_Y'

    world = bpy.data.worlds.new("World")
    world.use_nodes = True
    bg = world.node_tree.nodes.get("Background")
    if bg:
        bg.inputs["Color"].default_value = (0.55, 0.70, 0.90, 1.0)
        bg.inputs["Strength"].default_value = 0.5
    bpy.context.scene.world = world


def make_camera(location, name="Camera"):
    cam = bpy.data.cameras.new(name)
    cam.lens = 80  # Tighter lens for small object
    obj = bpy.data.objects.new(name, cam)
    obj.location = location
    bpy.context.collection.objects.link(obj)

    target = bpy.data.objects.new(f"{name}Target", None)
    target.location = (0, 0, STEM_HEIGHT + FROND_LENGTH * 0.3)
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
    """Remove render-only objects, join fern meshes, then export as GLB."""
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
    bpy.ops.object.join()
    fern = bpy.context.active_object
    fern.name = "Fern"

    # Collapse to a single material
    while len(fern.data.materials) > 1:
        fern.data.materials.pop(index=1)
    fern.data.materials[0].name = "Fern"

    # Reset origin to world origin (base at ground level)
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

    # Build fern
    stem_mat = make_material("Stem", STEM_COLOR)
    frond_mat = make_material("Frond", FROND_COLOR)

    stem, fronds = build_fern(stem_mat, frond_mat)

    # Render setup
    ground = add_ground()
    setup_lighting()

    os.makedirs(RENDER_DIR, exist_ok=True)

    cam_front = make_camera((1.5, -1.2, 0.8), "CamFront")
    render_to(cam_front, os.path.join(RENDER_DIR, "fern_front.png"))

    cam_side = make_camera((-0.5, -1.8, 0.6), "CamSide")
    render_to(cam_side, os.path.join(RENDER_DIR, "fern_side.png"))

    # Export if requested
    if do_export:
        export_glb([ground])

    # Summary
    total_height = STEM_HEIGHT + FROND_LENGTH * math.cos(math.radians(FROND_ANGLE))
    print(f"\n=== Fern Generation Complete ===")
    print(f"  Stem: {STEM_SIDES}-sided, r={STEM_RADIUS}m, h={STEM_HEIGHT}m")
    print(f"  Fronds: {FROND_COUNT} fronds, length={FROND_LENGTH}m, width={FROND_WIDTH}m, angle={FROND_ANGLE}deg")
    print(f"  Total height: ~{total_height:.2f}m")
    print(f"  Renders: {RENDER_DIR}/")
    if do_export:
        print(f"  GLB: {EXPORT_FILE}")


if __name__ == "__main__":
    main()
