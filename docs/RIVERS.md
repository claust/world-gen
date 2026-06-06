# River rendering

Rivers are a *global* phenomenon solved once per world load: a priority-flood
hydrology pass over a coarse toroidal grid bakes a per-cell **carve depth**,
**wetness** (`0..1`), and a normalized **downstream flow direction** into the
`RiverField` (`world_core::rivers`). Per-chunk terrain generation bilinearly
samples this field: carve cuts the channel into the heightmap, wetness and flow
ride along as per-vertex attributes on `ChunkTerrain` (`river`, `river_flow`,
plus a `has_river` flag).

Originally a river was just a blue *tint* painted onto the terrain albedo — it
read as wet dirt, not water. The current renderer draws an actual translucent
water surface in the carved channel with animated, direction-aware flow.

## Options, by effort vs. impact

### Option A — Riverbed tint only (superseded) ✅ (still present under the water)
Blend the terrain albedo toward a desaturated blue by `wetness` in
`terrain.wgsl`. No geometry, no transparency, no motion — the channel reads as
darker ground. Cheap, and it still provides the bed color *beneath* the water
surface, but on its own it was "hardly visible." Kept as the bed shading; the
water surface (B) sits on top.

### Option B — Real river water surface ✅ implemented (B2)
A dedicated translucent pass (`renderer_wgpu::river_pass` + `shaders/river.wgsl`)
builds a water mesh that sits in the channel — surface height = carved bed +
a wetness-scaled depth, so deeper channels fill more and the sheet self-levels.
It reuses the sea water's alpha-blend pipeline and shadow setup, gets fresnel
alpha + a sun glint, and only builds for chunks flagged `has_river` (the shader
discards the dry fringe per-fragment). This is what turns "wet dirt" into water.

- **B1** — extend the existing sea `WaterPass` to also emit river quads (one
  draw). Rejected: rivers want their own animation/shading and channel meshing.
- **B2** — a separate `RiverPass` (chosen): cleaner separation, own shader.

### Option C — Scrolling-noise flow ✅ implemented
The streaming cue, all in `river.wgsl`, driven by the per-vertex flow vector and
`frame.time.x` (real elapsed seconds, independent of `day_speed`):

- **Flow-aligned ripples** perturb the surface normal so the sun glint travels
  downstream (crests hold constant phase `u·k − t·s`, so they drift along +flow).
- **Advected "current" streaks** — gentle light/dark bands scrolled along the
  flow that read as moving water even when the glint doesn't point at the camera.

Procedural only (no textures, no foam). Verified moving: with camera and sun
frozen, ~135k px in the water region change over a 5 s gap.

### Option D — Flow maps (textures) ✅ implemented
Augments C's analytic sine ripples with a **sampled normal map advected along
the flow vector**, using the two-phase cross-fade technique (Valve's "Water Flow
Over Arbitrary Surfaces", Portal 2). The procedural sines visibly stretched and
swam around bends and confluences; the flow map fixes that and adds real
micro-surface detail. C's coarse analytic ripple is kept at reduced amplitude as
a large-scale undulation layer; the current streaks are unchanged.

What went into it:
- **Asset:** a small tiling water normal map, generated procedurally on the CPU
  (`renderer_wgpu::river_normal_texture`, same spirit as the terrain atlas — one
  modest 256² texture, not per-river art). Normals come from the gradient of a
  seamlessly-tiling FBM height field; a full CPU-built mip chain lets the fine
  ripples filter cleanly into the distance instead of aliasing.
- **Binding:** a texture + sampler at **group 3** of the river pass (groups 0–2
  are frame/material/shadow). The river vertex already carries `flow` and
  `wetness`; UVs are world `XZ × 0.16` (~6 m tile), `Repeat`-wrapped.
- **Advection:** sample the map twice at UVs offset along `flow` by
  `(phase − 0.5) · displace`, with two phases a half-cycle apart, cross-faded by
  a triangle wave `abs(1 − 2·frac(time·rate))`. Each phase only ever scrolls ±½ a
  cycle, so neither accumulates unbounded stretch — this is what stops the swim.
- **Lighting:** the sampled tangent-space normal's XY is treated as a surface
  slope and added to the coarse analytic slope, then the combined normal drives
  the existing diffuse/glint/fresnel. A distance fade eases the fine detail
  toward the coarse layer past ~80 m to kill grazing-angle ripple aliasing.

gen_key-neutral: no baked data, no geometry change — purely a runtime texture +
shader sampling. Verified in-engine on the largest world rivers (close-up shows
the rippled normal detail and clean bank fade; ~127k px change over a 5 s gap
with camera and sun frozen confirms it still streams downstream).

### Option F — Fresnel sky reflection ✅ implemented
The actual root cause of "only visible opposite the sun": the water body was lit
almost identically to the surrounding land (near-flat normals, same diffuse) and
painted nearly the same blue as the riverbed tint beneath it, so the only strong
cue was the **sun glint** — which by construction fires only when the mirror
reflection points at the eye. Real water reads as water from *any* angle because
it reflects the **sky**, bright and pale toward grazing angles. So `river.wgsl`
now picks a sky color along the reflected view ray (`material.sky_zenith` toward
`sky_horizon`) and blends the water body toward it by a Schlick-style fresnel
(floored so even top-down views pick up some sky). Goes dark for free at night as
the sky uniforms dim. This is what makes the channel pop off the land off-axis;
foam (E1) then adds the moving cue on top. gen_key-neutral (runtime shading only).

### Option E — Foam / whitewater ⏳ deferred (E1 tried, reverted)
Additive white foam, the single strongest readability cue — the eye locks onto
moving white streaks instantly. Two sub-parts at different cost:

- **E1 — Shoreline / edge foam ⏳ tried & reverted:** a first pass added two
  additive near-white bands advected along the flow frame (sharpened-sine wave
  crests + cross-rippled shoreline lapping). In-engine it read as discrete,
  too-circular white *spots* sliding down the channel rather than foam — the
  sharpened sine made round blobs, not streaks. Reverted; the **F** sky-reflection
  alone already made the river read clearly as water, which was the main goal. If
  revisited, foam wants a proper scrolling noise/texture mask (reusing D's group-3
  binding) and thin flow-elongated shapes, not an analytic sine threshold.
- **E2 — Whitewater on fast/steep water (more work):** foam concentrated where
  the current is **fast or steep** (rapids). This needs a **flow-speed signal we
  don't currently bake** — the flow field stores direction only (normalized); the
  drainage accumulation we compute is a proxy for river *size*, not speed.
  - **Prereq:** bake a per-cell **speed/turbulence** scalar into `RiverField` —
    e.g. from bed **slope** along the flow direction (steeper ⇒ faster ⇒ more
    foam), optionally × accumulation. Sample it per-vertex like `wetness`/`flow`
    and carry it on the river vertex. (Cheap to add to the existing solve; the
    `receiver`/heights are right there.)
  - **Render:** mask the foam texture by `speed`, advect along flow, add white
    with its own alpha. Optionally a touch of foam where flow vectors converge
    (confluences) since the bilinear flow length already dips there.

Effort: E1 small, E2 moderate (+ a baked speed grid → `gen_key`-neutral since it
doesn't change carved heights). Impact: very high — foam sells "rushing river."

## Recommended phasing

1. **A + B + C** — done. Water surface that visibly streams downhill.
2. **D** — done. Flow-map normal texture for convincing surface detail around
   bends; added the first texture binding (group 3) to the river pass, which E1
   can now reuse.
3. **F** — done. Fresnel sky reflection (the off-axis visibility fix); this alone
   made the river read clearly as water. E1 foam was tried here and reverted (read
   as circular spots — see Option E).
4. **E1 / E2** — foam: E1 needs a texture-based foam mask (not analytic sine);
   E2 whitewater additionally needs a baked flow-speed/slope scalar in `RiverField`.

D and E are independent of each other but both build on the flow field from C;
E2 is the only one that needs new baked data (a speed/slope grid).

## Implementation notes (A–C, current)

- **Flow field:** `compute_hydrology` keeps the `receiver` array (each cell's
  downhill neighbour) instead of discarding it after accumulation; `generate`
  converts it to a normalized world-space `(flow_x, flow_z)` per river cell.
  `RiverField::sample_flow(x, z)` bilinear-samples and renormalizes (the blend
  can shrink near confluences, so we don't trust the interpolated length).
- **gen_key:** carve/wetness/heights are unchanged, so the baked `world_base.bin`
  and `gen_key` are untouched — flow is additive runtime data the renderer reads;
  terrain geometry is identical.
- **ChunkTerrain:** gained `river_flow: Vec<[f32; 2]>` and `has_river: bool`,
  parallel to the existing `river` wetness. Populated in `world_core::terrain`;
  flow is only sampled where `wetness > RIVER_SURFACE_THRESHOLD`.
- **River mesh:** built per `has_river` chunk over the full terrain grid; surface
  y = `heights[i] + depth`, `depth = RIVER_MIN_DEPTH + wetness · RIVER_DEPTH_SCALE`
  (lifts the sheet off the bed, deeper toward the channel centre). Terrain heights
  reach the GPU as the carved values, so the surface aligns with the rendered bed.
- **Animation:** `river.wgsl` builds a flow-aligned frame (`f` downstream, `perp`
  across), travels ripples + light/dark current streaks along it, and fades alpha
  from the bank (`wetness`) and at glancing angles (fresnel). Uses `frame.time.x`,
  so it animates regardless of `day_speed`.
- **Flow map (D):** `river.wgsl` samples the tiling normal map
  (`renderer_wgpu::river_normal_texture`, group 3) twice along the per-vertex
  `flow` with the two-phase cross-fade, blends the result into the coarse
  analytic slope, and fades the fine detail by camera distance. This added
  micro-surface detail, fixed the bend/confluence swim, and — with mips +
  distance fade — smoothed the far grazing-angle banding C left behind.
- **Known gaps (E, not yet done):** no foam/whitewater. Shoreline foam (E1) can
  reuse D's group-3 texture binding and the per-vertex `wetness`; whitewater (E2)
  still needs a baked flow-speed/slope scalar in `RiverField`.
