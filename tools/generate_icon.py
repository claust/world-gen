"""Generate the FloraForge macOS app icon.

Flat reproduction of the FloraForge brand mark / favicon: a stacked two-tier
conifer in bright "forge green" on a solid dark-green tile. No gradients, glow,
or shading — flat fills, matching the favicon exactly. The favicon geometry
(viewBox 0..64) is the source of truth, scaled up to a crisp 1024px master and
supersampled for anti-aliased edges.

Favicon reference (web/index.html):
    rect 64x64 rx16            fill #0b2010   (dark green tile)
    upper triangle (32,12)(44,34)(20,34)      fill #3ecf6e
    lower triangle (32,24)(48,52)(16,52)      fill #3ecf6e
"""

from PIL import Image, ImageDraw
import os

SIZE = 1024
SS = 4  # supersampling factor for crisp edges
VIEWBOX = 64  # favicon coordinate space
# macOS icon grid (Big Sur+): the rounded "body" is 824/1024 of the canvas with
# transparent padding around it, corner radius ~185/824. Filling the whole
# canvas makes the icon render larger than every system icon in the dock.
TILE_FRAC = 824 / 1024
CORNER_RADIUS = 0.225  # fraction of the tile

# Brand palette (from web/index.html) — flat fills, no gradients.
GREEN = (62, 207, 110)  # #3ecf6e — forge green canopy
BG = (11, 32, 16)  # #0b2010 — dark green tile


def scaled(points, size):
    s = size / VIEWBOX
    return [(x * s, y * s) for x, y in points]


def draw_tree(draw, size):
    """Two stacked canopy triangles, per the favicon geometry."""
    upper = scaled([(32, 12), (44, 34), (20, 34)], size)
    lower = scaled([(32, 24), (48, 52), (16, 52)], size)
    for tri in (lower, upper):
        draw.polygon(tri, fill=GREEN)


def rounded_mask(size, radius):
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, size - 1, size - 1], radius=radius, fill=255
    )
    return mask


def render(size):
    big = size * SS
    # Render the tile + tree into the inset body, then center it on a
    # transparent canvas so the dock sizes it like other macOS icons.
    tile = round(big * TILE_FRAC)
    body = Image.new("RGBA", (tile, tile), BG + (255,))
    draw_tree(ImageDraw.Draw(body, "RGBA"), tile)
    body.putalpha(rounded_mask(tile, int(tile * CORNER_RADIUS)))

    canvas = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    offset = (big - tile) // 2
    canvas.paste(body, (offset, offset), body)
    return canvas.resize((size, size), Image.LANCZOS)


def main():
    img = render(SIZE)

    out_dir = os.path.join(os.path.dirname(__file__), "..", "assets", "icon")
    os.makedirs(out_dir, exist_ok=True)
    master_path = os.path.join(out_dir, "icon_1024.png")
    img.save(master_path, "PNG")
    print(f"Saved master icon: {master_path}")

    iconset_dir = os.path.join(out_dir, "world-gen.iconset")
    os.makedirs(iconset_dir, exist_ok=True)

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
        img.resize((size, size), Image.LANCZOS).save(
            os.path.join(iconset_dir, name), "PNG"
        )
    print(f"Generated iconset: {iconset_dir}")
    return iconset_dir


if __name__ == "__main__":
    iconset_path = main()
    print(f"\nRun: iconutil -c icns {iconset_path}")
