#!/usr/bin/env bun
/**
 * tools/plant-gen/render.ts
 *
 * Generates an SVG side-view visualization of a plant species from a JSON definition.
 *
 * Usage:
 *   bun tools/plant-gen/render.ts <species.json> [output.svg]
 *   bun tools/plant-gen/render.ts examples/oak.json
 *   bun tools/plant-gen/render.ts examples/spruce.json my-spruce.svg
 */

import { readFileSync, writeFileSync } from "fs";
import { basename, dirname, join } from "path";

// ─── Types ───────────────────────────────────────────────────────────────────

interface Vec2 {
  x: number;
  y: number;
}

interface BranchSegment {
  start: Vec2;
  end: Vec2;
  thickness: number; // average diameter for stroke-width
  depth: number;
}

interface FoliageBlob {
  center: Vec2;
  radius: number;
  hueShift: number;
  lightShift: number;
}

interface SpeciesJson {
  name: string;
  body_plan: {
    kind: string;
    stem_count: number;
    max_height: [number, number];
  };
  trunk: {
    taper: number;
    base_flare: number;
    straightness: number;
    thickness_ratio: number;
  };
  branching: {
    apical_dominance: number;
    max_depth: number;
    arrangement: { type: string; angle?: number; count?: number };
    branches_per_node: [number, number];
    insertion_angle: {
      base: [number, number];
      tip: [number, number];
    };
    length_profile: string;
    child_length_ratio: number;
    child_thickness_ratio: number;
    gravity_response: number;
    randomness: number;
  };
  crown: {
    shape: string;
    crown_base: number;
    aspect_ratio: number;
    density: number;
    asymmetry: number;
  };
  foliage: {
    style: string;
    leaf_size: [number, number];
    cluster_strategy: { type: string; count?: number };
    droop: number;
    coverage: number;
  };
  color: {
    bark: { h: number; s: number; l: number };
    leaf: { h: number; s: number; l: number };
    leaf_variance?: number;
  };
}

// ─── Seeded RNG (xorshift32) ─────────────────────────────────────────────────

class RNG {
  private s: number;
  constructor(seed: number) {
    this.s = (seed | 0) || 1;
  }
  next(): number {
    this.s ^= this.s << 13;
    this.s ^= this.s >> 17;
    this.s ^= this.s << 5;
    return (this.s >>> 0) / 0xffffffff;
  }
  range(a: number, b: number): number {
    return a + this.next() * (b - a);
  }
  int(a: number, b: number): number {
    return Math.floor(this.range(a, b + 0.999));
  }
  static hash(s: string): number {
    let h = 5381;
    for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
    return h;
  }
}

// ─── Color ───────────────────────────────────────────────────────────────────

function hslToHex(h: number, s: number, l: number): string {
  h = ((h % 360) + 360) % 360;
  s = Math.max(0, Math.min(1, s));
  l = Math.max(0, Math.min(1, l));
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  let r = 0,
    g = 0,
    b = 0;
  if (h < 60) {
    r = c;
    g = x;
  } else if (h < 120) {
    r = x;
    g = c;
  } else if (h < 180) {
    g = c;
    b = x;
  } else if (h < 240) {
    g = x;
    b = c;
  } else if (h < 300) {
    r = x;
    b = c;
  } else {
    r = c;
    b = x;
  }
  const hex = (v: number) =>
    Math.round((v + m) * 255)
      .toString(16)
      .padStart(2, "0");
  return `#${hex(r)}${hex(g)}${hex(b)}`;
}

function hslToRgba(h: number, s: number, l: number, a: number): string {
  h = ((h % 360) + 360) % 360;
  s = Math.max(0, Math.min(1, s));
  l = Math.max(0, Math.min(1, l));
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  let r = 0,
    g = 0,
    b = 0;
  if (h < 60) {
    r = c;
    g = x;
  } else if (h < 120) {
    r = x;
    g = c;
  } else if (h < 180) {
    g = c;
    b = x;
  } else if (h < 240) {
    g = x;
    b = c;
  } else if (h < 300) {
    r = x;
    b = c;
  } else {
    r = c;
    b = x;
  }
  const ri = Math.round((r + m) * 255);
  const gi = Math.round((g + m) * 255);
  const bi = Math.round((b + m) * 255);
  return `rgba(${ri},${gi},${bi},${a.toFixed(2)})`;
}

// ─── Crown Envelope ──────────────────────────────────────────────────────────

function isInsideCrown(
  shape: string,
  point: Vec2,
  treeHeight: number,
  crownBase: number,
  aspectRatio: number,
): boolean {
  const crownBottom = treeHeight * crownBase;
  const crownTop = treeHeight;
  const crownHeight = crownTop - crownBottom;
  if (crownHeight <= 0) return true;

  const crownCenterY = (crownTop + crownBottom) / 2;
  const crownRadiusY = crownHeight / 2;
  const crownRadiusX = crownRadiusY * aspectRatio;

  // Normalize point relative to crown
  const nx = point.x / crownRadiusX;
  const nyCenter = (point.y - crownCenterY) / crownRadiusY;
  const tInCrown = (point.y - crownBottom) / crownHeight; // 0=bottom, 1=top

  const slack = 1.15; // allow slight overflow for organic look

  switch (shape) {
    case "dome":
    case "oval":
      return nx * nx + nyCenter * nyCenter <= slack;

    case "conical": {
      if (tInCrown < -0.05 || tInCrown > 1.05) return false;
      const w = crownRadiusX * Math.max(0, 1 - tInCrown) * slack;
      return Math.abs(point.x) <= w;
    }

    case "columnar": {
      if (tInCrown < -0.05 || tInCrown > 1.05) return false;
      return Math.abs(point.x) <= crownRadiusX * 0.35 * slack;
    }

    case "vase": {
      if (tInCrown < -0.05 || tInCrown > 1.05) return false;
      const w = crownRadiusX * (0.3 + 0.7 * tInCrown) * slack;
      return Math.abs(point.x) <= w;
    }

    case "umbrella": {
      if (tInCrown < -0.05 || tInCrown > 1.05) return false;
      const w = crownRadiusX * (tInCrown > 0.6 ? 1.0 : 0.2 + 0.8 * tInCrown) * slack;
      return Math.abs(point.x) <= w;
    }

    case "weeping":
      // Wider, taller envelope to accommodate drooping branches
      return (
        (point.x / (crownRadiusX * 1.3)) ** 2 +
          ((point.y - crownCenterY) / (crownRadiusY * 1.4)) ** 2 <=
        1.5
      );

    case "fan_top":
      return tInCrown > 0.8;

    default:
      // "irregular" — no constraint
      return true;
  }
}

// ─── Length Profile ──────────────────────────────────────────────────────────

function lengthProfile(profile: string, t: number): number {
  switch (profile) {
    case "conical":
      return Math.max(0, 1 - t);
    case "dome":
      return Math.sin(t * Math.PI);
    case "columnar":
      return 0.6 + 0.4 * Math.sin(t * Math.PI);
    case "vase":
      return 0.3 + 0.7 * t;
    case "layered":
      return 0.4 + 0.6 * Math.abs(Math.sin(t * Math.PI * 3));
    default:
      return 1;
  }
}

// ─── Tree Generation ─────────────────────────────────────────────────────────

function generateTree(spec: SpeciesJson): {
  segments: BranchSegment[];
  foliage: FoliageBlob[];
  height: number;
} {
  const rng = new RNG(RNG.hash(spec.name));
  const segments: BranchSegment[] = [];
  const foliage: FoliageBlob[] = [];

  const height = rng.range(spec.body_plan.max_height[0], spec.body_plan.max_height[1]);
  const trunkRadius = height * spec.trunk.thickness_ratio;
  const stemCount = Math.max(1, spec.body_plan.stem_count);

  if (stemCount <= 1) {
    generateStem(spec, rng, { x: 0, y: 0 }, Math.PI / 2, height, trunkRadius, segments, foliage);
  } else {
    for (let i = 0; i < stemCount; i++) {
      const spread = ((i / (stemCount - 1)) - 0.5) * 1.0;
      const dir = Math.PI / 2 + spread * 0.4;
      const h = height * rng.range(0.65, 1.0);
      const r = trunkRadius * rng.range(0.4, 0.7);
      generateStem(spec, rng, { x: spread * 0.3, y: 0 }, dir, h, r, segments, foliage);
    }
  }

  return { segments, foliage, height };
}

function generateStem(
  spec: SpeciesJson,
  rng: RNG,
  base: Vec2,
  direction: number,
  height: number,
  baseRadius: number,
  segments: BranchSegment[],
  foliage: FoliageBlob[],
) {
  // Build trunk as a series of segments
  const nSeg = 6;
  const topRadius = baseRadius * (1 - spec.trunk.taper);
  const flareRadius = baseRadius * (1 + spec.trunk.base_flare);

  let dir = direction;
  let pos = { ...base };
  const trunkPts: Vec2[] = [{ ...pos }];

  for (let i = 0; i < nSeg; i++) {
    const t0 = i / nSeg;
    const t1 = (i + 1) / nSeg;

    // Wobble for non-straight trunks
    dir += (1 - spec.trunk.straightness) * rng.range(-0.06, 0.06);

    const segLen = height / nSeg;
    const next: Vec2 = {
      x: pos.x + Math.cos(dir) * segLen,
      y: pos.y + Math.sin(dir) * segLen,
    };

    // Thickness with base flare on first segment
    const r0 = i === 0 ? flareRadius : baseRadius + (topRadius - baseRadius) * t0;
    const r1 = baseRadius + (topRadius - baseRadius) * t1;

    segments.push({
      start: { ...pos },
      end: { ...next },
      thickness: (r0 + r1), // diameter = 2 * avg radius
      depth: 0,
    });

    pos = { ...next };
    trunkPts.push({ ...pos });
  }

  // Palm / fan_top: fronds only, no branching
  if (spec.crown.shape === "fan_top" || spec.foliage.style === "palm_frond") {
    generateFronds(spec, rng, pos, height, baseRadius * (1 - spec.trunk.taper), segments, foliage);
    return;
  }

  // Branch attachment points along the crown region of the trunk
  const crownStart = height * spec.crown.crown_base;
  const crownHeight = height - crownStart;
  if (crownHeight <= 0) return;

  const interNode = height * 0.065;
  const numNodes = Math.max(4, Math.ceil(crownHeight / interNode));

  let side = rng.next() > 0.5 ? 1 : -1;
  const asymBias = spec.crown.asymmetry * 0.3;

  for (let n = 0; n < numNodes; n++) {
    const tCrown = (n + 0.5) / numNodes;
    const branchY = crownStart + tCrown * crownHeight;
    const tTrunk = branchY / height;

    // Interpolate position on trunk
    const ti = tTrunk * nSeg;
    const idx = Math.min(Math.floor(ti), nSeg - 1);
    const frac = ti - idx;
    const origin: Vec2 = {
      x: trunkPts[idx].x + (trunkPts[idx + 1].x - trunkPts[idx].x) * frac,
      y: trunkPts[idx].y + (trunkPts[idx + 1].y - trunkPts[idx].y) * frac,
    };

    // Branch length from profile
    const profileScale = lengthProfile(spec.branching.length_profile, tCrown);
    const maxLen = crownHeight * spec.crown.aspect_ratio * 0.4;
    const baseLen = maxLen * profileScale * (1 - spec.branching.apical_dominance * 0.3);

    // Branch thickness
    const thickHere = baseRadius * 2 * (1 - tTrunk * spec.trunk.taper * 0.7);
    const branchThick = thickHere * spec.branching.child_thickness_ratio;

    const count = rng.int(spec.branching.branches_per_node[0], spec.branching.branches_per_node[1]);

    for (let b = 0; b < count; b++) {
      // Insertion angle gradient
      const angBase = rng.range(spec.branching.insertion_angle.base[0], spec.branching.insertion_angle.base[1]);
      const angTip = rng.range(spec.branching.insertion_angle.tip[0], spec.branching.insertion_angle.tip[1]);
      const insertDeg = angBase + (angTip - angBase) * tTrunk;
      const insertRad = (insertDeg * Math.PI) / 180;

      const randomDev = spec.branching.randomness * rng.range(-0.15, 0.15);
      const branchDir = direction + side * insertRad + randomDev;

      // Asymmetry: branches on the favored side are slightly longer
      const asymMul = side > 0 ? 1 + asymBias : 1 - asymBias;
      const len = baseLen * rng.range(0.7, 1.3) * asymMul;

      side *= -1;

      generateBranch(spec, rng, origin, branchDir, len, branchThick, 1, tTrunk, height, direction, segments, foliage);
    }
  }
}

function generateBranch(
  spec: SpeciesJson,
  rng: RNG,
  origin: Vec2,
  angle: number,
  length: number,
  thickness: number,
  depth: number,
  heightRatio: number,
  treeHeight: number,
  trunkDir: number,
  segments: BranchSegment[],
  foliage: FoliageBlob[],
) {
  if (length < 0.08 || thickness < 0.005) return;

  // Compute endpoint with gravity droop
  const dx = Math.cos(angle) * length;
  const dy = Math.sin(angle) * length;
  const gravDrop = spec.branching.gravity_response * length * length * 0.04;

  const end: Vec2 = {
    x: origin.x + dx,
    y: origin.y + dy - gravDrop,
  };

  // Crown envelope check
  if (!isInsideCrown(spec.crown.shape, end, treeHeight, spec.crown.crown_base, spec.crown.aspect_ratio)) {
    // Add a small foliage blob at boundary
    if (spec.foliage.style !== "none") {
      const r = rng.range(spec.foliage.leaf_size[0], spec.foliage.leaf_size[1]) * treeHeight * 0.06;
      foliage.push({
        center: { x: (origin.x + end.x) / 2, y: (origin.y + end.y) / 2 },
        radius: Math.max(r, 0.2),
        hueShift: rng.range(-15, 15),
        lightShift: rng.range(-0.08, 0.08),
      });
    }
    return;
  }

  // Record this segment
  segments.push({
    start: { ...origin },
    end: { ...end },
    thickness,
    depth,
  });

  // Terminal branch: add foliage and stop
  if (depth >= spec.branching.max_depth) {
    if (spec.foliage.style !== "none") {
      addFoliage(spec, rng, end, length, treeHeight, foliage);
    }
    return;
  }

  // Apical continuation (extends the branch forward)
  if (spec.branching.apical_dominance > 0.2) {
    const contLen = length * spec.branching.child_length_ratio * (0.5 + 0.5 * spec.branching.apical_dominance);
    const contThick = thickness * spec.branching.child_thickness_ratio;
    const contAngle = angle + spec.branching.randomness * rng.range(-0.08, 0.08);
    generateBranch(spec, rng, end, contAngle, contLen, contThick, depth + 1, heightRatio, treeHeight, trunkDir, segments, foliage);
  }

  // Lateral child branches
  const numChildren = rng.int(spec.branching.branches_per_node[0], spec.branching.branches_per_node[1]);
  let childSide = 1;

  for (let i = 0; i < numChildren; i++) {
    const spreadAngle = rng.range(0.3, 0.8);
    const childAngle = angle + childSide * spreadAngle + spec.branching.randomness * rng.range(-0.15, 0.15);
    const childLen = length * spec.branching.child_length_ratio * rng.range(0.6, 1.1);
    const childThick = thickness * spec.branching.child_thickness_ratio;

    childSide *= -1;

    generateBranch(spec, rng, end, childAngle, childLen, childThick, depth + 1, heightRatio, treeHeight, trunkDir, segments, foliage);
  }
}

function addFoliage(
  spec: SpeciesJson,
  rng: RNG,
  pos: Vec2,
  branchLen: number,
  treeHeight: number,
  foliage: FoliageBlob[],
) {
  const variance = spec.color.leaf_variance ?? 0.15;
  const sizeBase = treeHeight * 0.045 * (1 + spec.crown.density * 0.5);
  const strategy = spec.foliage.cluster_strategy;

  const blobCount =
    strategy.type === "dense_mass"
      ? Math.ceil(4 * spec.crown.density)
      : strategy.type === "clusters"
        ? (strategy.count ?? 3)
        : 1;

  const spread = strategy.type === "dense_mass" ? sizeBase * 1.2 : sizeBase * 0.6;

  for (let i = 0; i < blobCount; i++) {
    const r = sizeBase * rng.range(0.5, 1.3);
    foliage.push({
      center: {
        x: pos.x + rng.range(-spread, spread),
        y: pos.y + rng.range(-spread * 0.5, spread * 0.6),
      },
      radius: Math.max(r, 0.15),
      hueShift: rng.range(-1, 1) * variance * 100,
      lightShift: rng.range(-1, 1) * variance,
    });
  }
}

function generateFronds(
  spec: SpeciesJson,
  rng: RNG,
  apex: Vec2,
  treeHeight: number,
  topRadius: number,
  segments: BranchSegment[],
  foliage: FoliageBlob[],
) {
  const frondCount =
    spec.foliage.cluster_strategy.type === "ring"
      ? (spec.foliage.cluster_strategy.count ?? 16)
      : 14;

  const frondLength = treeHeight * 0.3;

  for (let i = 0; i < frondCount; i++) {
    // Project 3D ring to 2D side view: use sin for x-spread, cos for depth (ignored)
    const angle3d = (i / frondCount) * Math.PI * 2;
    const xSpread = Math.sin(angle3d);
    const depthFade = Math.abs(Math.cos(angle3d)); // fronds facing us/away are foreshortened

    const spreadAngle = xSpread * 1.2; // radians from vertical
    const droop = spec.foliage.droop * frondLength * 0.4;
    const effectiveLen = frondLength * (0.5 + 0.5 * (1 - depthFade * 0.4));

    const endX = apex.x + Math.sin(spreadAngle) * effectiveLen;
    const endY = apex.y + Math.cos(spreadAngle) * effectiveLen * 0.6 - droop;

    segments.push({
      start: { ...apex },
      end: { x: endX, y: endY },
      thickness: topRadius * 0.25 * (1 - depthFade * 0.3),
      depth: 1,
    });

    // Foliage along frond
    const variance = spec.color.leaf_variance ?? 0.15;
    for (let j = 0; j < 5; j++) {
      const ft = 0.25 + j * 0.15;
      const fx = apex.x + (endX - apex.x) * ft;
      const fy = apex.y + (endY - apex.y) * ft;
      foliage.push({
        center: { x: fx + rng.range(-0.3, 0.3), y: fy + rng.range(-0.2, 0.2) },
        radius: treeHeight * 0.03 * (1.2 - ft * 0.5),
        hueShift: rng.range(-1, 1) * variance * 80,
        lightShift: rng.range(-1, 1) * variance * 0.8,
      });
    }
  }
}

// ─── SVG Rendering ───────────────────────────────────────────────────────────

function renderSVG(
  spec: SpeciesJson,
  segments: BranchSegment[],
  foliage: FoliageBlob[],
  treeHeight: number,
): string {
  // Compute world bounding box
  let minX = 0,
    maxX = 0,
    minY = 0,
    maxY = 0;

  for (const seg of segments) {
    const r = seg.thickness;
    minX = Math.min(minX, seg.start.x - r, seg.end.x - r);
    maxX = Math.max(maxX, seg.start.x + r, seg.end.x + r);
    minY = Math.min(minY, seg.start.y - r, seg.end.y - r);
    maxY = Math.max(maxY, seg.start.y + r, seg.end.y + r);
  }
  for (const blob of foliage) {
    minX = Math.min(minX, blob.center.x - blob.radius);
    maxX = Math.max(maxX, blob.center.x + blob.radius);
    minY = Math.min(minY, blob.center.y - blob.radius);
    maxY = Math.max(maxY, blob.center.y + blob.radius);
  }

  // Add margins
  const margin = treeHeight * 0.15;
  const groundDepth = treeHeight * 0.12;
  minX -= margin;
  maxX += margin;
  minY = Math.min(minY, 0) - groundDepth;
  maxY += margin * 0.6;

  const worldW = maxX - minX;
  const worldH = maxY - minY;

  // SVG dimensions — scale to fit within 800px max dimension
  const maxPx = 800;
  const scale = maxPx / Math.max(worldW, worldH);
  const svgW = Math.round(worldW * scale);
  const svgH = Math.round(worldH * scale);

  // World → SVG coordinate transform (flip Y)
  const tx = (wx: number) => ((wx - minX) * scale).toFixed(2);
  const ty = (wy: number) => ((maxY - wy) * scale).toFixed(2);

  const groundSvgY = Number(ty(0));
  const lines: string[] = [];

  lines.push(`<svg xmlns="http://www.w3.org/2000/svg" width="${svgW}" height="${svgH}" viewBox="0 0 ${svgW} ${svgH}">`);

  // Sky gradient
  lines.push(`<defs>`);
  lines.push(`  <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1">`);
  lines.push(`    <stop offset="0%" stop-color="#87CEEB"/>`);
  lines.push(`    <stop offset="70%" stop-color="#d4e8f7"/>`);
  lines.push(`    <stop offset="100%" stop-color="#eef5db"/>`);
  lines.push(`  </linearGradient>`);
  lines.push(`  <linearGradient id="ground" x1="0" y1="0" x2="0" y2="1">`);
  lines.push(`    <stop offset="0%" stop-color="#5a7a3a"/>`);
  lines.push(`    <stop offset="100%" stop-color="#4a6830"/>`);
  lines.push(`  </linearGradient>`);
  lines.push(`</defs>`);

  // Background sky
  lines.push(`<rect width="${svgW}" height="${svgH}" fill="url(#sky)"/>`);

  // Ground
  const gndH = svgH - groundSvgY;
  if (gndH > 0) {
    lines.push(`<rect x="0" y="${groundSvgY.toFixed(1)}" width="${svgW}" height="${gndH.toFixed(1)}" fill="url(#ground)"/>`);
  }

  // Shadow ellipse on ground
  const shadowW = treeHeight * spec.crown.aspect_ratio * 0.5 * scale;
  const shadowH = shadowW * 0.15;
  const shadowCx = Number(tx(0));
  lines.push(
    `<ellipse cx="${shadowCx.toFixed(1)}" cy="${(groundSvgY + 2).toFixed(1)}" rx="${shadowW.toFixed(1)}" ry="${shadowH.toFixed(1)}" fill="rgba(0,0,0,0.15)"/>`,
  );

  // Sort segments: depth ascending (trunk first), then by thickness descending
  const sortedSegs = [...segments].sort((a, b) => {
    if (a.depth !== b.depth) return a.depth - b.depth;
    return b.thickness - a.thickness;
  });

  // Draw branches
  const bark = spec.color.bark;
  for (const seg of sortedSegs) {
    // Slightly vary bark color by depth
    const depthDarken = seg.depth * 0.03;
    const color = hslToHex(bark.h, bark.s, Math.max(0.1, bark.l - depthDarken));
    const sw = Math.max(seg.thickness * scale, 1);
    lines.push(
      `<line x1="${tx(seg.start.x)}" y1="${ty(seg.start.y)}" x2="${tx(seg.end.x)}" y2="${ty(seg.end.y)}" stroke="${color}" stroke-width="${sw.toFixed(1)}" stroke-linecap="round"/>`,
    );
  }

  // Sort foliage: by y ascending (lower blobs drawn first, behind upper)
  const sortedFol = [...foliage].sort((a, b) => a.center.y - b.center.y);

  // Draw foliage
  const leaf = spec.color.leaf;
  const baseOpacity = 0.45 + spec.crown.density * 0.4;

  for (const blob of sortedFol) {
    const h = leaf.h + blob.hueShift;
    const l = Math.max(0.15, Math.min(0.6, leaf.l + blob.lightShift));
    const color = hslToRgba(h, leaf.s, l, baseOpacity);
    const r = Math.max(blob.radius * scale, 2);
    lines.push(
      `<circle cx="${tx(blob.center.x)}" cy="${ty(blob.center.y)}" r="${r.toFixed(1)}" fill="${color}"/>`,
    );
  }

  // Title label
  const fontSize = Math.max(14, Math.round(svgW * 0.03));
  lines.push(
    `<text x="${fontSize}" y="${fontSize * 1.5}" font-family="system-ui, sans-serif" font-size="${fontSize}" fill="#333" font-weight="600">${escapeXml(spec.name)}</text>`,
  );

  // Params subtitle
  const subSize = Math.round(fontSize * 0.65);
  const subtitle = `${spec.crown.shape} · ${spec.branching.length_profile} · dominance ${spec.branching.apical_dominance}`;
  lines.push(
    `<text x="${fontSize}" y="${fontSize * 1.5 + subSize * 1.4}" font-family="system-ui, sans-serif" font-size="${subSize}" fill="#666">${escapeXml(subtitle)}</text>`,
  );

  lines.push(`</svg>`);
  return lines.join("\n");
}

function escapeXml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

// ─── Main ────────────────────────────────────────────────────────────────────

function main() {
  const args = process.argv.slice(2);
  if (args.length === 0) {
    console.log("Usage: bun tools/plant-gen/render.ts <species.json> [output.svg]");
    process.exit(0);
  }

  const inputPath = args[0];
  const outputPath = args[1] ?? inputPath.replace(/\.json$/i, ".svg");

  // Read and parse species JSON
  let spec: SpeciesJson;
  try {
    spec = JSON.parse(readFileSync(inputPath, "utf-8"));
  } catch (err) {
    console.error(`Failed to read ${inputPath}: ${err instanceof Error ? err.message : err}`);
    process.exit(1);
  }

  // Generate
  const { segments, foliage, height } = generateTree(spec);

  // Render SVG
  const svg = renderSVG(spec, segments, foliage, height);

  // Write output
  writeFileSync(outputPath, svg, "utf-8");

  // Summary
  console.log(`Plant: ${spec.name}`);
  console.log(`  Height: ${height.toFixed(1)}m | Crown: ${spec.crown.shape} | Profile: ${spec.branching.length_profile}`);
  console.log(`  Dominance: ${spec.branching.apical_dominance} | Gravity: ${spec.branching.gravity_response} | Depth: ${spec.branching.max_depth}`);
  console.log(`  Generated: ${segments.length} segments, ${foliage.length} foliage blobs`);
  console.log(`  Output: ${outputPath}`);
}

main();
