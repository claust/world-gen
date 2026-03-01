#!/usr/bin/env bun
/**
 * tools/plant-gen/render.ts
 *
 * Generates SVG or GLB visualization of a plant species from a JSON definition.
 * Generation is fully 3D; SVG output projects to a 2D side view.
 *
 * Usage:
 *   bun tools/plant-gen/render.ts <species.json> [output.svg|.glb] [--format svg|glb]
 *   bun tools/plant-gen/render.ts examples/oak.json                 # → oak.svg
 *   bun tools/plant-gen/render.ts examples/oak.json oak.glb          # → oak.glb
 *   bun tools/plant-gen/render.ts examples/oak.json --format glb     # → oak.glb
 */

import { readFileSync, writeFileSync } from "fs";

// ─── Types ───────────────────────────────────────────────────────────────────

interface Vec3 { x: number; y: number; z: number }

interface BranchSegment {
  start: Vec3;
  end: Vec3;
  startRadius: number;
  endRadius: number;
  depth: number;
}

interface FoliageBlob {
  center: Vec3;
  radius: number;
  hueShift: number;
  lightShift: number;
}

interface SpeciesJson {
  name: string;
  body_plan: { kind: string; stem_count: number; max_height: [number, number] };
  trunk: { taper: number; base_flare: number; straightness: number; thickness_ratio: number };
  branching: {
    apical_dominance: number; max_depth: number;
    arrangement: { type: string; angle?: number; count?: number };
    branches_per_node: [number, number];
    insertion_angle: { base: [number, number]; tip: [number, number] };
    length_profile: string; child_length_ratio: number; child_thickness_ratio: number;
    gravity_response: number; randomness: number;
  };
  crown: { shape: string; crown_base: number; aspect_ratio: number; density: number; asymmetry: number };
  foliage: { style: string; leaf_size: [number, number]; cluster_strategy: { type: string; count?: number }; droop: number; coverage: number }; // coverage: reserved, not yet used by renderer
  color: { bark: { h: number; s: number; l: number }; leaf: { h: number; s: number; l: number }; leaf_variance?: number };
}

// ─── Vec3 Math ───────────────────────────────────────────────────────────────

const v3 = (x: number, y: number, z: number): Vec3 => ({ x, y, z });
const add3 = (a: Vec3, b: Vec3): Vec3 => v3(a.x + b.x, a.y + b.y, a.z + b.z);
const sub3 = (a: Vec3, b: Vec3): Vec3 => v3(a.x - b.x, a.y - b.y, a.z - b.z);
const scale3 = (v: Vec3, s: number): Vec3 => v3(v.x * s, v.y * s, v.z * s);
const dot3 = (a: Vec3, b: Vec3): number => a.x * b.x + a.y * b.y + a.z * b.z;
const len3 = (v: Vec3): number => Math.sqrt(dot3(v, v));
const normalize3 = (v: Vec3): Vec3 => { const l = len3(v); return l > 1e-8 ? scale3(v, 1 / l) : v3(0, 1, 0); };
const cross3 = (a: Vec3, b: Vec3): Vec3 => v3(
  a.y * b.z - a.z * b.y, a.z * b.x - a.x * b.z, a.x * b.y - a.y * b.x,
);
const lerp3 = (a: Vec3, b: Vec3, t: number): Vec3 => add3(scale3(a, 1 - t), scale3(b, t));

/** Compute a branch direction by tilting parentDir by insertAngle, rotated around it by rotAngle */
function branchDir3D(parentDir: Vec3, insertAngleRad: number, rotRad: number): Vec3 {
  const ref = Math.abs(parentDir.y) < 0.95 ? v3(0, 1, 0) : v3(1, 0, 0);
  const p1 = normalize3(cross3(parentDir, ref));
  const p2 = cross3(parentDir, p1);
  const rotPerp = add3(scale3(p1, Math.cos(rotRad)), scale3(p2, Math.sin(rotRad)));
  return normalize3(add3(
    scale3(parentDir, Math.cos(insertAngleRad)),
    scale3(rotPerp, Math.sin(insertAngleRad)),
  ));
}

// ─── Seeded RNG (xorshift32) ─────────────────────────────────────────────────

class RNG {
  private s: number;
  constructor(seed: number) { this.s = (seed | 0) || 1; }
  next(): number {
    this.s ^= this.s << 13; this.s ^= this.s >> 17; this.s ^= this.s << 5;
    return (this.s >>> 0) / 0x100000000;
  }
  range(a: number, b: number): number { return a + this.next() * (b - a); }
  int(a: number, b: number): number { return a + Math.floor(this.next() * (b - a + 1)); }
  static hash(s: string): number {
    let h = 5381;
    for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) | 0;
    return h;
  }
}

// ─── Color ───────────────────────────────────────────────────────────────────

function hslToSrgb(h: number, s: number, l: number): [number, number, number] {
  h = ((h % 360) + 360) % 360;
  s = Math.max(0, Math.min(1, s));
  l = Math.max(0, Math.min(1, l));
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  let r = 0, g = 0, b = 0;
  if (h < 60) { r = c; g = x; }
  else if (h < 120) { r = x; g = c; }
  else if (h < 180) { g = c; b = x; }
  else if (h < 240) { g = x; b = c; }
  else if (h < 300) { r = x; b = c; }
  else { r = c; b = x; }
  return [r + m, g + m, b + m];
}

function hslToHex(h: number, s: number, l: number): string {
  const [r, g, b] = hslToSrgb(h, s, l);
  const hex = (v: number) => Math.round(v * 255).toString(16).padStart(2, "0");
  return `#${hex(r)}${hex(g)}${hex(b)}`;
}

function hslToRgba(h: number, s: number, l: number, a: number): string {
  const [r, g, b] = hslToSrgb(h, s, l);
  return `rgba(${Math.round(r * 255)},${Math.round(g * 255)},${Math.round(b * 255)},${a.toFixed(2)})`;
}

function hslToLinear(h: number, s: number, l: number): [number, number, number] {
  const [r, g, b] = hslToSrgb(h, s, l);
  const toL = (c: number) => c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  return [toL(r), toL(g), toL(b)];
}

// ─── Crown Envelope (3D — uses horizontal distance from trunk axis) ──────────

function isInsideCrown(
  shape: string, point: Vec3, treeHeight: number, crownBase: number, aspectRatio: number,
): boolean {
  const crownBottom = treeHeight * crownBase;
  const crownHeight = treeHeight - crownBottom;
  if (crownHeight <= 0) return true;

  const crownCenterY = (treeHeight + crownBottom) / 2;
  const crownRadiusY = crownHeight / 2;
  const crownRadiusH = crownRadiusY * aspectRatio;

  const hDist = Math.sqrt(point.x * point.x + point.z * point.z);
  const nh = hDist / crownRadiusH;
  const nv = (point.y - crownCenterY) / crownRadiusY;
  const tInCrown = (point.y - crownBottom) / crownHeight;
  const slack = 1.15;

  switch (shape) {
    case "dome":
    case "oval":
      return nh * nh + nv * nv <= slack;
    case "conical":
      if (tInCrown < -0.05 || tInCrown > 1.05) return false;
      return hDist <= crownRadiusH * Math.max(0, 1 - tInCrown) * slack;
    case "columnar":
      if (tInCrown < -0.05 || tInCrown > 1.05) return false;
      return hDist <= crownRadiusH * 0.35 * slack;
    case "vase":
      if (tInCrown < -0.05 || tInCrown > 1.05) return false;
      return hDist <= crownRadiusH * (0.3 + 0.7 * tInCrown) * slack;
    case "umbrella":
      if (tInCrown < -0.05 || tInCrown > 1.05) return false;
      return hDist <= crownRadiusH * (tInCrown > 0.6 ? 1.0 : 0.2 + 0.8 * tInCrown) * slack;
    case "weeping":
      return (hDist / (crownRadiusH * 1.3)) ** 2 + ((point.y - crownCenterY) / (crownRadiusY * 1.4)) ** 2 <= 1.5;
    case "fan_top":
      return tInCrown > 0.8;
    default:
      return true;
  }
}

// ─── Length Profile ──────────────────────────────────────────────────────────

function lengthProfile(profile: string, t: number): number {
  switch (profile) {
    case "conical": return Math.max(0, 1 - t);
    case "dome": return Math.sin(t * Math.PI);
    case "columnar": return 0.6 + 0.4 * Math.sin(t * Math.PI);
    case "vase": return 0.3 + 0.7 * t;
    case "layered": return 0.4 + 0.6 * Math.abs(Math.sin(t * Math.PI * 3));
    default: return 1;
  }
}

// ─── 3D Tree Generation ─────────────────────────────────────────────────────

function generateTree(spec: SpeciesJson): { segments: BranchSegment[]; foliage: FoliageBlob[]; height: number } {
  const rng = new RNG(RNG.hash(spec.name));
  const segments: BranchSegment[] = [];
  const foliage: FoliageBlob[] = [];
  const height = rng.range(spec.body_plan.max_height[0], spec.body_plan.max_height[1]);
  const trunkRadius = height * spec.trunk.thickness_ratio;
  const stemCount = Math.max(1, spec.body_plan.stem_count);

  if (stemCount <= 1) {
    generateStem(spec, rng, v3(0, 0, 0), v3(0, 1, 0), height, trunkRadius, segments, foliage);
  } else {
    for (let i = 0; i < stemCount; i++) {
      const a = (i / stemCount) * Math.PI * 2;
      const spread = 0.3;
      const base = v3(Math.cos(a) * spread, 0, Math.sin(a) * spread);
      const dir = normalize3(v3(Math.cos(a) * 0.2, 1, Math.sin(a) * 0.2));
      const h = height * rng.range(0.65, 1.0);
      const r = trunkRadius * rng.range(0.4, 0.7);
      generateStem(spec, rng, base, dir, h, r, segments, foliage);
    }
  }

  return { segments, foliage, height };
}

function generateStem(
  spec: SpeciesJson, rng: RNG, base: Vec3, direction: Vec3,
  height: number, baseRadius: number,
  segments: BranchSegment[], foliage: FoliageBlob[],
) {
  const nSeg = 6;
  const topRadius = baseRadius * (1 - spec.trunk.taper);
  const flareRadius = baseRadius * (1 + spec.trunk.base_flare);

  let dir = { ...direction };
  let pos = { ...base };
  const trunkPts: Vec3[] = [{ ...pos }];
  const trunkDirs: Vec3[] = [{ ...dir }];

  for (let i = 0; i < nSeg; i++) {
    const t0 = i / nSeg;
    const t1 = (i + 1) / nSeg;
    const wobble = (1 - spec.trunk.straightness) * 0.06;
    dir = normalize3(v3(dir.x + rng.range(-wobble, wobble), dir.y, dir.z + rng.range(-wobble, wobble)));

    const segLen = height / nSeg;
    const next = add3(pos, scale3(dir, segLen));
    const r0 = i === 0 ? flareRadius : baseRadius + (topRadius - baseRadius) * t0;
    const r1 = baseRadius + (topRadius - baseRadius) * t1;

    segments.push({ start: { ...pos }, end: { ...next }, startRadius: r0, endRadius: r1, depth: 0 });
    pos = { ...next };
    trunkPts.push({ ...pos });
    trunkDirs.push({ ...dir });
  }

  if (spec.crown.shape === "fan_top" || spec.foliage.style === "palm_frond") {
    generateFronds(spec, rng, pos, height, topRadius, segments, foliage);
    return;
  }

  const crownStart = height * spec.crown.crown_base;
  const crownHeight = height - crownStart;
  if (crownHeight <= 0) return;

  const interNode = height * 0.065;
  const numNodes = Math.max(4, Math.ceil(crownHeight / interNode));

  let arrangementRot = rng.next() * Math.PI * 2;
  const arrStep = spec.branching.arrangement.type === "spiral"
    ? ((spec.branching.arrangement.angle ?? 137.5) * Math.PI / 180)
    : spec.branching.arrangement.type === "opposite" ? Math.PI : 0;

  for (let n = 0; n < numNodes; n++) {
    const tCrown = (n + 0.5) / numNodes;
    const tTrunk = (crownStart + tCrown * crownHeight) / height;

    const ti = tTrunk * nSeg;
    const idx = Math.min(Math.floor(ti), nSeg - 1);
    const frac = ti - idx;
    const origin = lerp3(trunkPts[idx], trunkPts[idx + 1], frac);
    const localDir = normalize3(lerp3(trunkDirs[idx], trunkDirs[idx + 1], frac));

    const profileScale = lengthProfile(spec.branching.length_profile, tCrown);
    const maxLen = crownHeight * spec.crown.aspect_ratio * 0.4;
    const baseLen = maxLen * profileScale * (1 - spec.branching.apical_dominance * 0.3);
    const thickHere = baseRadius * (1 - tTrunk * spec.trunk.taper * 0.7);
    const branchThick = thickHere * spec.branching.child_thickness_ratio;

    const count = rng.int(spec.branching.branches_per_node[0], spec.branching.branches_per_node[1]);

    if (spec.branching.arrangement.type === "whorled") {
      arrangementRot = rng.next() * Math.PI * 2;
    }

    for (let b = 0; b < count; b++) {
      const angBase = rng.range(spec.branching.insertion_angle.base[0], spec.branching.insertion_angle.base[1]);
      const angTip = rng.range(spec.branching.insertion_angle.tip[0], spec.branching.insertion_angle.tip[1]);
      const insertDeg = angBase + (angTip - angBase) * tTrunk;
      const insertRad = insertDeg * Math.PI / 180;
      const randomRot = spec.branching.randomness * rng.range(-0.3, 0.3);
      const brDir = branchDir3D(localDir, insertRad, arrangementRot + randomRot);

      const len = baseLen * rng.range(0.7, 1.3);

      if (spec.branching.arrangement.type === "whorled") {
        arrangementRot += (2 * Math.PI) / count;
      } else if (arrStep > 0) {
        arrangementRot += arrStep;
      } else {
        arrangementRot = rng.next() * Math.PI * 2;
      }

      generateBranch(spec, rng, origin, brDir, len, branchThick, 1, tTrunk, height, segments, foliage);
    }
  }
}

function generateBranch(
  spec: SpeciesJson, rng: RNG, origin: Vec3, direction: Vec3,
  length: number, thickness: number, depth: number,
  heightRatio: number, treeHeight: number,
  segments: BranchSegment[], foliage: FoliageBlob[],
) {
  if (length < 0.08 || thickness < 0.005) return;

  const rawEnd = add3(origin, scale3(direction, length));
  const gravDrop = spec.branching.gravity_response * length * length * 0.04;
  const end = v3(rawEnd.x, rawEnd.y - gravDrop, rawEnd.z);

  if (!isInsideCrown(spec.crown.shape, end, treeHeight, spec.crown.crown_base, spec.crown.aspect_ratio)) {
    if (spec.foliage.style !== "none") {
      const mid = scale3(add3(origin, end), 0.5);
      const r = rng.range(spec.foliage.leaf_size[0], spec.foliage.leaf_size[1]) * treeHeight * 0.06;
      foliage.push({ center: mid, radius: Math.max(r, 0.2), hueShift: rng.range(-15, 15), lightShift: rng.range(-0.08, 0.08) });
    }
    return;
  }

  const endR = Math.max(thickness * (1 - spec.trunk.taper * 0.3), 0.005);
  segments.push({ start: { ...origin }, end: { ...end }, startRadius: thickness, endRadius: endR, depth });

  if (depth >= spec.branching.max_depth) {
    if (spec.foliage.style !== "none") addFoliage(spec, rng, end, length, treeHeight, foliage);
    return;
  }

  const effDir = normalize3(sub3(end, origin));

  if (spec.branching.apical_dominance > 0.2) {
    const contLen = length * spec.branching.child_length_ratio * (0.5 + 0.5 * spec.branching.apical_dominance);
    const contThick = thickness * spec.branching.child_thickness_ratio;
    const contDir = normalize3(add3(effDir, v3(
      rng.range(-0.05, 0.05) * spec.branching.randomness, 0,
      rng.range(-0.05, 0.05) * spec.branching.randomness,
    )));
    generateBranch(spec, rng, end, contDir, contLen, contThick, depth + 1, heightRatio, treeHeight, segments, foliage);
  }

  const numChildren = rng.int(spec.branching.branches_per_node[0], spec.branching.branches_per_node[1]);
  let childRot = rng.next() * Math.PI * 2;
  for (let i = 0; i < numChildren; i++) {
    const spreadAngle = rng.range(0.3, 0.8);
    const randomRot = spec.branching.randomness * rng.range(-0.3, 0.3);
    const childDir = branchDir3D(effDir, spreadAngle, childRot + randomRot);
    const childLen = length * spec.branching.child_length_ratio * rng.range(0.6, 1.1);
    const childThick = thickness * spec.branching.child_thickness_ratio;
    childRot += Math.PI * 0.8 + rng.range(-0.2, 0.2);
    generateBranch(spec, rng, end, childDir, childLen, childThick, depth + 1, heightRatio, treeHeight, segments, foliage);
  }
}

function addFoliage(spec: SpeciesJson, rng: RNG, pos: Vec3, _branchLen: number, treeHeight: number, foliage: FoliageBlob[]) {
  const variance = spec.color.leaf_variance ?? 0.15;
  const sizeBase = treeHeight * 0.045 * (1 + spec.crown.density * 0.5);
  const strategy = spec.foliage.cluster_strategy;
  const blobCount = strategy.type === "dense_mass" ? Math.ceil(4 * spec.crown.density)
    : strategy.type === "clusters" ? (strategy.count ?? 3) : 1;
  const spread = strategy.type === "dense_mass" ? sizeBase * 1.2 : sizeBase * 0.6;

  for (let i = 0; i < blobCount; i++) {
    foliage.push({
      center: v3(pos.x + rng.range(-spread, spread), pos.y + rng.range(-spread * 0.5, spread * 0.6), pos.z + rng.range(-spread, spread)),
      radius: Math.max(sizeBase * rng.range(0.5, 1.3), 0.15),
      hueShift: rng.range(-1, 1) * variance * 100,
      lightShift: rng.range(-1, 1) * variance,
    });
  }
}

function generateFronds(spec: SpeciesJson, rng: RNG, apex: Vec3, treeHeight: number, topRadius: number, segments: BranchSegment[], foliage: FoliageBlob[]) {
  const frondCount = spec.foliage.cluster_strategy.type === "ring" ? (spec.foliage.cluster_strategy.count ?? 16) : 14;
  const frondLength = treeHeight * 0.3;
  const variance = spec.color.leaf_variance ?? 0.15;

  for (let i = 0; i < frondCount; i++) {
    const angle = (i / frondCount) * Math.PI * 2;
    const droop = spec.foliage.droop * frondLength * 0.4;
    const dx = Math.cos(angle) * frondLength * 0.8;
    const dz = Math.sin(angle) * frondLength * 0.8;
    const dy = frondLength * 0.3 - droop;
    const end = v3(apex.x + dx, apex.y + dy, apex.z + dz);

    segments.push({ start: { ...apex }, end, startRadius: topRadius * 0.25, endRadius: topRadius * 0.05, depth: 1 });

    for (let j = 0; j < 5; j++) {
      const ft = 0.25 + j * 0.15;
      foliage.push({
        center: v3(apex.x + dx * ft + rng.range(-0.3, 0.3), apex.y + dy * ft, apex.z + dz * ft + rng.range(-0.3, 0.3)),
        radius: treeHeight * 0.03 * (1.2 - ft * 0.5),
        hueShift: rng.range(-1, 1) * variance * 80,
        lightShift: rng.range(-1, 1) * variance * 0.8,
      });
    }
  }
}

// ─── SVG Rendering (projects 3D → 2D side view using x,y) ───────────────────

function renderSVG(spec: SpeciesJson, segments: BranchSegment[], foliage: FoliageBlob[], treeHeight: number): string {
  let minX = 0, maxX = 0, minY = 0, maxY = 0;
  for (const seg of segments) {
    const r = seg.startRadius + seg.endRadius;
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

  const margin = treeHeight * 0.15;
  const groundDepth = treeHeight * 0.12;
  minX -= margin; maxX += margin;
  minY = Math.min(minY, 0) - groundDepth;
  maxY += margin * 0.6;

  const worldW = maxX - minX, worldH = maxY - minY;
  const maxPx = 800;
  const scale = maxPx / Math.max(worldW, worldH);
  const svgW = Math.round(worldW * scale), svgH = Math.round(worldH * scale);
  const tx = (wx: number) => ((wx - minX) * scale).toFixed(2);
  const ty = (wy: number) => ((maxY - wy) * scale).toFixed(2);
  const groundSvgY = Number(ty(0));
  const L: string[] = [];

  L.push(`<svg xmlns="http://www.w3.org/2000/svg" width="${svgW}" height="${svgH}" viewBox="0 0 ${svgW} ${svgH}">`);
  L.push(`<defs>`);
  L.push(`  <linearGradient id="sky" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#87CEEB"/><stop offset="70%" stop-color="#d4e8f7"/><stop offset="100%" stop-color="#eef5db"/></linearGradient>`);
  L.push(`  <linearGradient id="ground" x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stop-color="#5a7a3a"/><stop offset="100%" stop-color="#4a6830"/></linearGradient>`);
  L.push(`</defs>`);
  L.push(`<rect width="${svgW}" height="${svgH}" fill="url(#sky)"/>`);
  const gndH = svgH - groundSvgY;
  if (gndH > 0) L.push(`<rect x="0" y="${groundSvgY.toFixed(1)}" width="${svgW}" height="${gndH.toFixed(1)}" fill="url(#ground)"/>`);

  const shadowW = treeHeight * spec.crown.aspect_ratio * 0.5 * scale;
  L.push(`<ellipse cx="${Number(tx(0)).toFixed(1)}" cy="${(groundSvgY + 2).toFixed(1)}" rx="${shadowW.toFixed(1)}" ry="${(shadowW * 0.15).toFixed(1)}" fill="rgba(0,0,0,0.15)"/>`);

  const sortedSegs = [...segments].sort((a, b) => a.depth !== b.depth ? a.depth - b.depth : b.startRadius - a.startRadius);
  const bark = spec.color.bark;
  for (const seg of sortedSegs) {
    const color = hslToHex(bark.h, bark.s, Math.max(0.1, bark.l - seg.depth * 0.03));
    const sw = Math.max((seg.startRadius + seg.endRadius) * scale, 1);
    L.push(`<line x1="${tx(seg.start.x)}" y1="${ty(seg.start.y)}" x2="${tx(seg.end.x)}" y2="${ty(seg.end.y)}" stroke="${color}" stroke-width="${sw.toFixed(1)}" stroke-linecap="round"/>`);
  }

  const sortedFol = [...foliage].sort((a, b) => a.center.y - b.center.y);
  const leaf = spec.color.leaf;
  const baseOpacity = 0.45 + spec.crown.density * 0.4;
  for (const blob of sortedFol) {
    const color = hslToRgba(leaf.h + blob.hueShift, leaf.s, Math.max(0.15, Math.min(0.6, leaf.l + blob.lightShift)), baseOpacity);
    L.push(`<circle cx="${tx(blob.center.x)}" cy="${ty(blob.center.y)}" r="${Math.max(blob.radius * scale, 2).toFixed(1)}" fill="${color}"/>`);
  }

  const fs = Math.max(14, Math.round(svgW * 0.03));
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  L.push(`<text x="${fs}" y="${fs * 1.5}" font-family="system-ui,sans-serif" font-size="${fs}" fill="#333" font-weight="600">${esc(spec.name)}</text>`);
  const sub = `${spec.crown.shape} · ${spec.branching.length_profile} · dominance ${spec.branching.apical_dominance}`;
  L.push(`<text x="${fs}" y="${fs * 1.5 + fs * 0.65 * 1.4}" font-family="system-ui,sans-serif" font-size="${Math.round(fs * 0.65)}" fill="#666">${esc(sub)}</text>`);
  L.push(`</svg>`);
  return L.join("\n");
}

// ─── GLB Rendering ───────────────────────────────────────────────────────────

const CYL_SIDES = 8;

const PHI = (1 + Math.sqrt(5)) / 2;
const ICO_V = [
  v3(-1, PHI, 0), v3(1, PHI, 0), v3(-1, -PHI, 0), v3(1, -PHI, 0),
  v3(0, -1, PHI), v3(0, 1, PHI), v3(0, -1, -PHI), v3(0, 1, -PHI),
  v3(PHI, 0, -1), v3(PHI, 0, 1), v3(-PHI, 0, -1), v3(-PHI, 0, 1),
].map(p => normalize3(p));
const ICO_F = [
  [0,11,5],[0,5,1],[0,1,7],[0,7,10],[0,10,11],
  [1,5,9],[5,11,4],[11,10,2],[10,7,6],[7,1,8],
  [3,9,4],[3,4,2],[3,2,6],[3,6,8],[3,8,9],
  [4,9,5],[2,4,11],[6,2,10],[8,6,7],[9,8,1],
];

function addCylinder(
  start: Vec3, end: Vec3, startR: number, endR: number,
  color: [number, number, number, number],
  pos: number[], norm: number[], col: number[], idx: number[],
) {
  const baseIdx = pos.length / 3;
  const dir = normalize3(sub3(end, start));
  const ref = Math.abs(dir.y) < 0.95 ? v3(0, 1, 0) : v3(1, 0, 0);
  const right = normalize3(cross3(dir, ref));
  const fwd = cross3(right, dir);

  for (let ring = 0; ring < 2; ring++) {
    const center = ring === 0 ? start : end;
    const radius = ring === 0 ? startR : endR;
    for (let i = 0; i < CYL_SIDES; i++) {
      const a = (i / CYL_SIDES) * Math.PI * 2;
      const ca = Math.cos(a), sa = Math.sin(a);
      const nx = right.x * ca + fwd.x * sa;
      const ny = right.y * ca + fwd.y * sa;
      const nz = right.z * ca + fwd.z * sa;
      pos.push(center.x + nx * radius, center.y + ny * radius, center.z + nz * radius);
      norm.push(nx, ny, nz);
      col.push(color[0], color[1], color[2], color[3]);
    }
  }

  for (let i = 0; i < CYL_SIDES; i++) {
    const i0 = baseIdx + i;
    const i1 = baseIdx + (i + 1) % CYL_SIDES;
    const i2 = baseIdx + CYL_SIDES + i;
    const i3 = baseIdx + CYL_SIDES + (i + 1) % CYL_SIDES;
    idx.push(i0, i2, i1, i1, i2, i3);
  }
}

function addIcosahedron(
  center: Vec3, radius: number, color: [number, number, number, number],
  pos: number[], norm: number[], col: number[], idx: number[],
) {
  const baseIdx = pos.length / 3;
  for (const v of ICO_V) {
    pos.push(center.x + v.x * radius, center.y + v.y * radius, center.z + v.z * radius);
    norm.push(v.x, v.y, v.z);
    col.push(color[0], color[1], color[2], color[3]);
  }
  for (const f of ICO_F) {
    idx.push(baseIdx + f[0], baseIdx + f[1], baseIdx + f[2]);
  }
}

function renderGLB(spec: SpeciesJson, segments: BranchSegment[], foliage: FoliageBlob[]): Buffer {
  const posArr: number[] = [];
  const normArr: number[] = [];
  const colArr: number[] = [];
  const idxArr: number[] = [];

  // Bark color (linear RGB)
  const [br, bg, bb] = hslToLinear(spec.color.bark.h, spec.color.bark.s, spec.color.bark.l);
  const barkColor: [number, number, number, number] = [br, bg, bb, 1.0];

  for (const seg of segments) {
    const depthDarken = 1 - seg.depth * 0.05;
    const c: [number, number, number, number] = [br * depthDarken, bg * depthDarken, bb * depthDarken, 1.0];
    addCylinder(seg.start, seg.end, seg.startRadius, seg.endRadius, c, posArr, normArr, colArr, idxArr);
  }

  // Leaf colors
  const leafBase = spec.color.leaf;
  const variance = spec.color.leaf_variance ?? 0.15;

  for (const blob of foliage) {
    const h = leafBase.h + blob.hueShift;
    const l = Math.max(0.15, Math.min(0.6, leafBase.l + blob.lightShift));
    const [lr, lg, lb] = hslToLinear(h, leafBase.s, l);
    addIcosahedron(blob.center, blob.radius, [lr, lg, lb, 1.0], posArr, normArr, colArr, idxArr);
  }

  const vertexCount = posArr.length / 3;
  if (vertexCount > 65535) {
    console.warn(`  Warning: ${vertexCount} vertices exceeds uint16 limit. GLB may have rendering issues.`);
  }

  // Compute bounds
  let minP = v3(Infinity, Infinity, Infinity);
  let maxP = v3(-Infinity, -Infinity, -Infinity);
  for (let i = 0; i < posArr.length; i += 3) {
    minP = v3(Math.min(minP.x, posArr[i]), Math.min(minP.y, posArr[i + 1]), Math.min(minP.z, posArr[i + 2]));
    maxP = v3(Math.max(maxP.x, posArr[i]), Math.max(maxP.y, posArr[i + 1]), Math.max(maxP.z, posArr[i + 2]));
  }

  // Build typed arrays
  const positions = new Float32Array(posArr);
  const normals = new Float32Array(normArr);
  const colors = new Float32Array(colArr);
  const indices = new Uint16Array(idxArr.map(i => i & 0xFFFF));

  const posBytes = positions.byteLength;
  const normBytes = normals.byteLength;
  const colBytes = colors.byteLength;
  const idxBytes = indices.byteLength;
  const idxPad = (4 - (idxBytes % 4)) % 4;
  const binTotal = posBytes + normBytes + colBytes + idxBytes + idxPad;

  const indexCount = indices.length;

  const gltf = {
    asset: { version: "2.0", generator: "plant-gen" },
    scene: 0,
    scenes: [{ nodes: [0] }],
    nodes: [{ mesh: 0, name: spec.name }],
    meshes: [{
      primitives: [{
        attributes: { POSITION: 0, NORMAL: 1, COLOR_0: 2 },
        indices: 3,
        material: 0,
        mode: 4,
      }],
    }],
    materials: [{
      pbrMetallicRoughness: { baseColorFactor: [1, 1, 1, 1], metallicFactor: 0.0, roughnessFactor: 0.9 },
      name: "Plant",
    }],
    accessors: [
      { bufferView: 0, componentType: 5126, count: vertexCount, type: "VEC3", min: [minP.x, minP.y, minP.z], max: [maxP.x, maxP.y, maxP.z] },
      { bufferView: 1, componentType: 5126, count: vertexCount, type: "VEC3" },
      { bufferView: 2, componentType: 5126, count: vertexCount, type: "VEC4" },
      { bufferView: 3, componentType: 5123, count: indexCount, type: "SCALAR" },
    ],
    bufferViews: [
      { buffer: 0, byteOffset: 0, byteLength: posBytes, target: 34962 },
      { buffer: 0, byteOffset: posBytes, byteLength: normBytes, target: 34962 },
      { buffer: 0, byteOffset: posBytes + normBytes, byteLength: colBytes, target: 34962 },
      { buffer: 0, byteOffset: posBytes + normBytes + colBytes, byteLength: idxBytes, target: 34963 },
    ],
    buffers: [{ byteLength: binTotal }],
  };

  let jsonStr = JSON.stringify(gltf);
  const jsonPad = (4 - (jsonStr.length % 4)) % 4;
  jsonStr += " ".repeat(jsonPad);
  const jsonBuf = Buffer.from(jsonStr, "utf-8");

  const totalLength = 12 + 8 + jsonBuf.length + 8 + binTotal;
  const out = Buffer.alloc(totalLength);
  let off = 0;

  // GLB header
  out.writeUInt32LE(0x46546C67, off); off += 4; // "glTF"
  out.writeUInt32LE(2, off); off += 4;           // version
  out.writeUInt32LE(totalLength, off); off += 4; // total length

  // JSON chunk
  out.writeUInt32LE(jsonBuf.length, off); off += 4;
  out.writeUInt32LE(0x4E4F534A, off); off += 4; // "JSON"
  jsonBuf.copy(out, off); off += jsonBuf.length;

  // BIN chunk
  out.writeUInt32LE(binTotal, off); off += 4;
  out.writeUInt32LE(0x004E4942, off); off += 4; // "BIN\0"
  Buffer.from(positions.buffer, positions.byteOffset, positions.byteLength).copy(out, off); off += posBytes;
  Buffer.from(normals.buffer, normals.byteOffset, normals.byteLength).copy(out, off); off += normBytes;
  Buffer.from(colors.buffer, colors.byteOffset, colors.byteLength).copy(out, off); off += colBytes;
  Buffer.from(indices.buffer, indices.byteOffset, indices.byteLength).copy(out, off);

  return out;
}

// ─── Main ────────────────────────────────────────────────────────────────────

function main() {
  const args = [...process.argv.slice(2)];

  const fmtIdx = args.indexOf("--format");
  let format: string | undefined;
  if (fmtIdx !== -1) {
    format = args[fmtIdx + 1];
    args.splice(fmtIdx, 2);
  }

  if (args.length === 0) {
    console.log("Usage: bun tools/plant-gen/render.ts <species.json> [output.svg|.glb] [--format svg|glb]");
    process.exit(0);
  }

  const inputPath = args[0];
  let outputPath = args[1];
  if (!outputPath) {
    const ext = format === "glb" ? ".glb" : ".svg";
    outputPath = /\.json$/i.test(inputPath) ? inputPath.replace(/\.json$/i, ext) : `${inputPath}${ext}`;
  }
  const isGlb = format === "glb" || outputPath.endsWith(".glb");

  let spec: SpeciesJson;
  try {
    spec = JSON.parse(readFileSync(inputPath, "utf-8"));
  } catch (err) {
    console.error(`Failed to read ${inputPath}: ${err instanceof Error ? err.message : err}`);
    process.exit(1);
  }

  const { segments, foliage, height } = generateTree(spec);

  if (isGlb) {
    const glb = renderGLB(spec, segments, foliage);
    writeFileSync(outputPath, glb);
  } else {
    writeFileSync(outputPath, renderSVG(spec, segments, foliage, height), "utf-8");
  }

  const vertCount = isGlb ? segments.length * CYL_SIDES * 2 + foliage.length * 12 : 0;
  console.log(`Plant: ${spec.name}`);
  console.log(`  Height: ${height.toFixed(1)}m | Crown: ${spec.crown.shape} | Profile: ${spec.branching.length_profile}`);
  console.log(`  Dominance: ${spec.branching.apical_dominance} | Gravity: ${spec.branching.gravity_response} | Depth: ${spec.branching.max_depth}`);
  console.log(`  Generated: ${segments.length} segments, ${foliage.length} foliage blobs${isGlb ? ` (${vertCount} vertices)` : ""}`);
  console.log(`  Output: ${outputPath}`);
}

main();
