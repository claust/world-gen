"""Generate a macOS app icon for world-gen.

Stylized low-poly tree on a sky/terrain background,
matching the procedural terrain renderer aesthetic.
"""

from PIL import Image, ImageDraw
import math
import os

SIZE = 1024
MARGIN = 80  # breathing room from edges
CORNER_RADIUS = 180  # macOS-style rounded square


def lerp_color(c1, c2, t):
    """Linearly interpolate between two RGB colors."""
    return tuple(int(a + (b - a) * t) for a, b in zip(c1, c2))


def draw_rounded_rect_mask(size, radius):
    """Create a rounded rectangle mask."""
    mask = Image.new("L", (size, size), 0)
    draw = ImageDraw.Draw(mask)
    draw.rounded_rectangle([0, 0, size - 1, size - 1], radius=radius, fill=255)
    return mask


def draw_background(img):
    """Draw a gradient sky-to-terrain background."""
    draw = ImageDraw.Draw(img)
    sky_top = (60, 130, 190)
    sky_bottom = (140, 190, 220)
    ground_top = (75, 130, 60)
    ground_bottom = (55, 100, 45)

    horizon = int(SIZE * 0.65)

    for y in range(SIZE):
        if y < horizon:
            t = y / horizon
            color = lerp_color(sky_top, sky_bottom, t)
        else:
            t = (y - horizon) / (SIZE - horizon)
            color = lerp_color(ground_top, ground_bottom, t)
        draw.line([(0, y), (SIZE, y)], fill=color)


def draw_terrain_bumps(draw, horizon_y):
    """Draw subtle rolling hills at the horizon."""
    hill_color_back = (85, 140, 70)
    hill_color_front = (65, 120, 50)

    # Back hills
    points = [(0, horizon_y + 40)]
    for x in range(0, SIZE + 1, 8):
        y_offset = math.sin(x * 0.008) * 30 + math.sin(x * 0.02) * 12
        points.append((x, horizon_y + 20 + y_offset))
    points.append((SIZE, SIZE))
    points.append((0, SIZE))
    draw.polygon(points, fill=hill_color_back)

    # Front hills
    points = [(0, horizon_y + 60)]
    for x in range(0, SIZE + 1, 8):
        y_offset = math.sin(x * 0.012 + 1.5) * 25 + math.sin(x * 0.025 + 0.7) * 15
        points.append((x, horizon_y + 50 + y_offset))
    points.append((SIZE, SIZE))
    points.append((0, SIZE))
    draw.polygon(points, fill=hill_color_front)


def draw_tree(draw, cx, base_y, scale=1.0):
    """Draw a stylized conifer tree (layered triangles + trunk)."""
    trunk_w = int(22 * scale)
    trunk_h = int(80 * scale)
    trunk_color = (90, 65, 40)

    # Trunk
    draw.rectangle(
        [cx - trunk_w // 2, base_y - trunk_h, cx + trunk_w // 2, base_y],
        fill=trunk_color,
    )

    # Canopy layers (bottom to top, wider to narrower)
    layers = [
        {"width": 220, "height": 140, "color": (40, 110, 45)},
        {"width": 180, "height": 130, "color": (50, 130, 55)},
        {"width": 140, "height": 120, "color": (55, 145, 60)},
        {"width": 95, "height": 105, "color": (65, 160, 70)},
    ]

    layer_y = base_y - trunk_h + int(20 * scale)
    overlap = int(55 * scale)

    for layer in layers:
        w = int(layer["width"] * scale)
        h = int(layer["height"] * scale)

        triangle = [
            (cx - w // 2, layer_y),
            (cx + w // 2, layer_y),
            (cx, layer_y - h),
        ]
        draw.polygon(triangle, fill=layer["color"])

        # Light edge highlight on the right side
        highlight = lerp_color(layer["color"], (200, 230, 200), 0.25)
        highlight_tri = [
            (cx, layer_y - h),
            (cx + w // 2, layer_y),
            (cx + w // 4, layer_y),
        ]
        draw.polygon(highlight_tri, fill=highlight)

        layer_y -= h - overlap


def draw_small_tree(draw, cx, base_y, scale=0.35):
    """Draw a small background tree."""
    trunk_w = int(12 * scale * 2)
    trunk_h = int(50 * scale * 2)
    trunk_color = (70, 55, 35)

    draw.rectangle(
        [cx - trunk_w // 2, base_y - trunk_h, cx + trunk_w // 2, base_y],
        fill=trunk_color,
    )

    layers = [
        {"width": 160, "height": 100, "color": (35, 95, 38)},
        {"width": 120, "height": 90, "color": (42, 110, 48)},
        {"width": 80, "height": 80, "color": (50, 125, 55)},
    ]

    s = scale * 2
    layer_y = base_y - trunk_h + int(15 * s)
    overlap = int(40 * s)

    for layer in layers:
        w = int(layer["width"] * s)
        h = int(layer["height"] * s)
        triangle = [
            (cx - w // 2, layer_y),
            (cx + w // 2, layer_y),
            (cx, layer_y - h),
        ]
        draw.polygon(triangle, fill=layer["color"])
        layer_y -= h - overlap


def main():
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))

    # Draw background
    draw_background(img)
    draw = ImageDraw.Draw(img)

    horizon_y = int(SIZE * 0.65)
    draw_terrain_bumps(draw, horizon_y)

    # Small background trees
    draw_small_tree(draw, 180, horizon_y + 25, scale=0.30)
    draw_small_tree(draw, 750, horizon_y + 15, scale=0.25)
    draw_small_tree(draw, 870, horizon_y + 30, scale=0.32)

    # Main tree (center, prominent)
    draw_tree(draw, SIZE // 2, horizon_y + 40, scale=1.15)

    # Apply rounded rectangle mask (macOS style)
    mask = draw_rounded_rect_mask(SIZE, CORNER_RADIUS)
    img.putalpha(mask)

    # Save the 1024x1024 master icon
    out_dir = os.path.join(os.path.dirname(__file__), "..", "assets", "icon")
    os.makedirs(out_dir, exist_ok=True)
    master_path = os.path.join(out_dir, "icon_1024.png")
    img.save(master_path, "PNG")
    print(f"Saved master icon: {master_path}")

    # Generate iconset for macOS
    iconset_dir = os.path.join(out_dir, "world-gen.iconset")
    os.makedirs(iconset_dir, exist_ok=True)

    # macOS iconset required sizes: 16, 32, 128, 256, 512 (1x and 2x)
    sizes = [
        ("icon_16x16.png", 16),
        ("icon_16x16@2x.png", 32),
        ("icon_32x32.png", 32),
        ("icon_32x32@2x.png", 64),
        ("icon_128x128.png", 128),
        ("icon_128x128@2x.png", 256),
        ("icon_256x256.png", 256),
        ("icon_256x256@2x.png", 512),
        ("icon_512x512.png", 512),
        ("icon_512x512@2x.png", 1024),
    ]

    for name, size in sizes:
        resized = img.resize((size, size), Image.LANCZOS)
        resized.save(os.path.join(iconset_dir, name), "PNG")

    print(f"Generated iconset: {iconset_dir}")
    return iconset_dir


if __name__ == "__main__":
    iconset_path = main()
    print(f"\nRun: iconutil -c icns {iconset_path}")
