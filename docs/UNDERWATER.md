# Underwater rendering

When the camera descends below `SEA_LEVEL`, nothing in the renderer currently
changes: the water surface becomes a semi-transparent sheet above the camera and
the terrain keeps rendering with its normal grass/biome textures, full-daylight
fog, and the sky gradient still draws in the distance. The result reads as "the
real world seen through a window" rather than "submerged."

The plumbing needed already exists: `camera_position` is in the per-frame uniform
(group 0) available to every fragment shader, `SEA_LEVEL` is tracked in
`world_core::chunk` and threaded through to `WaterPass`, and there is already a
distance-based fog mix in `terrain.wgsl`, `water.wgsl`, `instanced.wgsl`, and
`shrub_billboard.wgsl`. So most of this work is about *driving existing knobs
differently* when submerged.

## Options, by effort vs. impact

### Option A — Underwater fog + global tint (the big win, cheap) ✅ implemented
Add a single per-frame `submerge_factor` (0 above water, ramping to 1 just below
`SEA_LEVEL`) and branch the existing fog math on it:

- **Swap the fog color** from the sky-horizon color to a deep blue-green murk.
- **Collapse the fog distance** dramatically — underwater visibility is ~tens of
  metres, not the full chunk-load radius. Water absorbs light fast.
- **Depth-tint** the lit color toward the murk even at close range, so nearby
  surfaces lose their warmth (red is absorbed first underwater).

This alone transforms the feel: everything fades into murk, the world goes
blue-green, the horizon disappears. ~80% of the effect for ~20% of the work, and
it touches no geometry — just shader fog math plus one uniform value.

### Option B — Underwater lighting
Dim and blue-shift the sun and ambient when submerged. The hemisphere ambient and
sun term get scaled down and pushed toward blue, attenuating with depth so deep
terrain goes nearly black. Small change; pairs naturally with A.

### Option C — Different terrain coloring/texture below the waterline
- **C1 — Tint underwater (cheap):** where a terrain fragment's world Y is below
  `SEA_LEVEL`, blend its albedo toward a murky silt/sand color. No new assets;
  catches the actual seabed regardless of biome.
- **C2 — Real seabed textures (more work):** add sand/silt/seaweed/rock tiles to
  the terrain atlas and a "submerged" biome selection in `terrain_gen.wgsl` keyed
  on terrain height vs. sea level. More authentic, but new art + biome logic.
  Keyed on terrain height (not camera), so the seabed looks right viewed from
  above through clear shallow water too.

### Option D — Handle the surface-from-below & the sky
- The **sky pass** still draws a daylight gradient in the distance underwater.
  Skip it when submerged or override its output to the deep-water color, and make
  the **clear color** follow suit.
- The **water surface viewed from below** should look like a bright, rippling
  ceiling, not the same top-down fresnel sheet. Flip the alpha logic for the
  underside, and make sure the water mesh isn't back-face culled.

### Option E — Polish (later)
Full-screen blue vignette/overlay when submerged, animated caustics on the
seabed, subtle screen-distortion wobble, bubbles. Pure juice — do last.

## Recommended phasing

1. **A + B + C1** — all shader-side, one new uniform, no assets. This is where the
   "I'm actually underwater" feeling comes from.
2. **D** — close the sky/surface gaps.
3. **C2** — a genuinely different seabed, if desired.
4. **E** — polish.

The single most important piece is **A**: the murky short-range fog and
blue-green absorption is what the eye reads as "underwater" more than anything.

## Implementation notes (Option A)

- `submerge_factor` is computed CPU-side in `WorldRenderer::update_frame` from the
  camera Y vs. `SEA_LEVEL` (`clamp((sea_level - camera.y) / RAMP, 0, 1)`, ramped
  over ~1.5 m so crossing the surface is smooth, not a hard pop). It is packed
  into the unused `FrameUniform.time.z` slot — no uniform-layout change.
- A shared `scene_fog(color, world_pos)` helper lives in `lighting.wgsl` (which is
  concatenated ahead of `terrain.wgsl`, `water.wgsl`, `instanced.wgsl`, and now
  `shrub_billboard.wgsl`). It blends between the existing sky-colored air fog and
  an underwater murk fog by `submerge_factor`, replacing the duplicated 5-line fog
  block that previously lived in each shader.
- The underwater murk tint is dimmed by `material.ambient.x` so it goes dark at
  night for free.
- **Known gap (Option D, not in this phase):** the sky pass still renders a bright
  gradient in the distance and the water surface seen from below still uses the
  top-down look. Tracked for a follow-up.
