"""Generate the HUD plant-readout brand mark (`assets/hud/plant_count.png`).

The flat FloraForge spruce — the same two-tier conifer as the app icon / favicon —
rendered in a darker forge green on a transparent canvas, sized so the renderer can
seat the live population count inside its lower canopy. The HUD draws it translucently
in the top-left corner.

Favicon geometry (viewBox 0..64), the source of truth for the silhouette:
    upper triangle (32,12)(44,34)(20,34)
    lower triangle (32,24)(48,52)(16,52)

The canvas width must stay a multiple of 64: the renderer uploads the image with
`bytes_per_row = width * 4`, which wgpu requires to be 256-byte aligned.
"""

from PIL import Image, ImageDraw
import os

W, H = 512, 441  # width multiple of 64 for the texture row pitch; aspect ~1.16
SS = 4           # supersample for crisp anti-aliased edges
GREEN = (34, 140, 76)  # #228c4c — darker forge green for HUD legibility over sky
VFRAC = 0.80     # tree height as a fraction of the canvas (leaves breathing room)

UPPER = [(32, 12), (44, 34), (20, 34)]
LOWER = [(32, 24), (48, 52), (16, 52)]
MINX, MAXX, MINY, MAXY = 16, 48, 12, 52


def render():
    img = Image.new("RGBA", (W * SS, H * SS), (0, 0, 0, 0))
    d = ImageDraw.Draw(img, "RGBA")
    scale = (H * SS * VFRAC) / (MAXY - MINY)
    tw, th = (MAXX - MINX) * scale, (MAXY - MINY) * scale
    ox, oy = (W * SS - tw) / 2, (H * SS - th) / 2
    for tri in (LOWER, UPPER):  # lower first so the upper tier overlaps cleanly
        pts = [((x - MINX) * scale + ox, (y - MINY) * scale + oy) for x, y in tri]
        d.polygon(pts, fill=GREEN)
    return img.resize((W, H), Image.LANCZOS)


def main():
    out = os.path.join(os.path.dirname(__file__), "..", "assets", "hud", "plant_count.png")
    render().save(out, "PNG")
    print(f"Saved HUD brand mark: {out}")


if __name__ == "__main__":
    main()
