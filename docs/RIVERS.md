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

### Option D — Flow maps (textures) ⏳ deferred
Replace/augment C's analytic sine ripples with a **sampled normal (or dU/dV)
map advected along the flow vector**, using the two-phase cross-fade technique
(Valve's "Water Flow Over Arbitrary Surfaces", Portal 2). The procedural sines
visibly stretch and swim around bends and confluences; flow maps fix that and
add real micro-surface detail.

What goes into it:
- **Asset:** a small tiling normal/dU-dV water texture (same spirit as the biome
  atlas — one modest texture, not per-river art).
- **Binding:** add a texture + sampler to the river pass (new bind group, or
  fold into an existing slot). The river vertex already carries `flow` and
  `wetness`; UVs come from world `XZ × scale`.
- **Advection:** sample the map twice at UVs offset by `flow · phase`, with two
  phases a half-cycle apart, cross-faded by a triangle wave of `frac(time·rate)`.
  This keeps the texture from accumulating unbounded stretch as it scrolls — each
  phase resets before it distorts too far.
- **Use the sampled normal** for lighting/glint instead of (or blended with) the
  analytic ripple normal; keep C's current streaks as a coarse large-scale layer
  over the fine texture.

Prereqs: none beyond what's baked — the normalized flow field already exists.
Effort: moderate (asset + one binding + shader sampling). Impact: high — this is
what makes the surface read as genuinely flowing rather than gently rippling.

### Option E — Foam / whitewater ⏳ deferred
Additive white foam, the single strongest readability cue — the eye locks onto
moving white streaks instantly. Two sub-parts at different cost:

- **E1 — Shoreline / edge foam (cheap):** a band of foam where the surface meets
  the bank, keyed on the **wetness gradient** (foam where `wetness` is near the
  dry cutoff). Needs no new data — wetness is already per-vertex — and a small
  scrolling foam noise/texture advected along flow. Reads as a river edge lapping
  its banks. Good first slice; pairs with D's texture binding.
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
2. **D** — flow maps for convincing surface detail around bends. Adds the first
   texture binding to the river pass, which E1 can then reuse.
3. **E1** — shoreline foam (reuses D's texture binding, no new baked data).
4. **E2** — whitewater, once a baked flow-speed/slope scalar is added to
   `RiverField`.

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
- **Known gaps (D/E, not in this phase):** no textured surface detail (sines
  stretch around tight bends) and no foam; far-distance ripples show mild
  grazing-angle banding that a flow-map normal + distance fade would smooth.
