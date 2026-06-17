#!/usr/bin/env bun

import { readFileSync } from "node:fs";

const DEFAULT_API = "http://127.0.0.1:7777";
const TIMEOUT_MS = 5000;

// --- Arg parsing ---

function parseArgs(argv: string[]): { command: string; flags: Record<string, string> } {
  const args = argv.slice(2);
  const command = args[0] ?? "";
  const flags: Record<string, string> = {};

  for (let i = 1; i < args.length; i++) {
    const arg = args[i];
    if (arg.startsWith("--") && i + 1 < args.length) {
      flags[arg.slice(2)] = args[++i];
    }
  }

  return { command, flags };
}

function requireFlag(flags: Record<string, string>, name: string): string {
  const value = flags[name];
  if (value === undefined) {
    die(`missing required flag --${name}`);
  }
  return value;
}

function requireFloat(flags: Record<string, string>, name: string): number {
  const raw = requireFlag(flags, name);
  const n = Number(raw);
  if (!Number.isFinite(n)) die(`--${name} must be a number, got "${raw}"`);
  return n;
}

function requireInt(flags: Record<string, string>, name: string): number {
  const raw = requireFlag(flags, name);
  const n = Number(raw);
  if (!Number.isInteger(n)) die(`--${name} must be an integer, got "${raw}"`);
  return n;
}

function optionalFloat(flags: Record<string, string>, name: string): number | undefined {
  const raw = flags[name];
  if (raw === undefined) return undefined;
  const n = Number(raw);
  if (!Number.isFinite(n)) die(`--${name} must be a number, got "${raw}"`);
  return n;
}

function die(message: string): never {
  process.stderr.write(`error: ${message}\n`);
  process.exit(1);
}

// --- API helpers ---

function wsUrl(apiBase: string): string {
  const parsed = new URL(apiBase);
  parsed.protocol = parsed.protocol === "https:" ? "wss:" : "ws:";
  parsed.pathname = "/ws";
  return parsed.toString();
}

function commandId(): string {
  return `cli-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

async function sendAndWait(
  apiBase: string,
  command: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  const id = commandId();

  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      ws.close();
      reject(new Error("timeout waiting for command response"));
    }, TIMEOUT_MS);

    const ws = new WebSocket(wsUrl(apiBase));

    ws.onopen = async () => {
      const res = await fetch(`${apiBase}/api/command`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ id, ...command }),
      });
      if (!res.ok) {
        clearTimeout(timer);
        ws.close();
        reject(new Error(`HTTP ${res.status}: ${await res.text()}`));
      }
    };

    ws.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data as string);
        if (data.type === "command_applied" && data.payload?.id === id) {
          clearTimeout(timer);
          ws.close();
          resolve(data.payload);
        }
      } catch {
        // ignore non-matching messages
      }
    };

    ws.onerror = () => {
      clearTimeout(timer);
      reject(new Error("WebSocket connection failed"));
    };
  });
}

// --- Commands ---

async function cmdState(apiBase: string) {
  const res = await fetch(`${apiBase}/api/state`);
  if (!res.ok) die(`HTTP ${res.status}: ${await res.text()}`);
  const data = await res.json();
  console.log(JSON.stringify(data, null, 2));
}

async function cmdScreenshot(apiBase: string) {
  const result = await sendAndWait(apiBase, { type: "take_screenshot" });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdSave(apiBase: string) {
  const result = await sendAndWait(apiBase, { type: "save_world" });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdSetDaySpeed(apiBase: string, flags: Record<string, string>) {
  const value = requireFloat(flags, "value");
  const result = await sendAndWait(apiBase, { type: "set_day_speed", value });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdSetTime(apiBase: string, flags: Record<string, string>) {
  const hour = requireFloat(flags, "hour");
  const result = await sendAndWait(apiBase, { type: "set_time", hour });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdSetCameraPosition(apiBase: string, flags: Record<string, string>) {
  const x = requireFloat(flags, "x");
  const y = requireFloat(flags, "y");
  const z = requireFloat(flags, "z");
  const result = await sendAndWait(apiBase, { type: "set_camera_position", x, y, z });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdSetCameraLook(apiBase: string, flags: Record<string, string>) {
  const yaw = requireFloat(flags, "yaw");
  const pitch = requireFloat(flags, "pitch");
  const result = await sendAndWait(apiBase, { type: "set_camera_look", yaw, pitch });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdMapRightClick(apiBase: string, flags: Record<string, string>) {
  const u = requireFloat(flags, "u");
  const v = requireFloat(flags, "v");
  if (u < 0 || u > 1 || v < 0 || v > 1) die(`--u and --v must be in range 0..1`);
  const result = await sendAndWait(apiBase, { type: "map_right_click", u, v });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdSaveFavorite(apiBase: string, flags: Record<string, string>) {
  const slot = requireInt(flags, "slot");
  if (slot < 1 || slot > 5) die(`--slot must be an integer in range 1..5`);
  const result = await sendAndWait(apiBase, { type: "save_favorite", slot });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdRecallFavorite(apiBase: string, flags: Record<string, string>) {
  const slot = requireInt(flags, "slot");
  if (slot < 1 || slot > 5) die(`--slot must be an integer in range 1..5`);
  const result = await sendAndWait(apiBase, { type: "recall_favorite", slot });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdFindNearest(apiBase: string, flags: Record<string, string>) {
  const kind = requireFlag(flags, "kind");
  if (kind !== "house" && kind !== "tree" && kind !== "fern") die(`--kind must be "house", "tree", or "fern"`);
  const result = await sendAndWait(apiBase, { type: "find_nearest", kind });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdLookAt(apiBase: string, flags: Record<string, string>) {
  const object_id = requireFlag(flags, "id");
  const distance = optionalFloat(flags, "distance");
  const cmd: Record<string, unknown> = { type: "look_at_object", object_id };
  if (distance !== undefined) cmd.distance = distance;
  const result = await sendAndWait(apiBase, cmd);
  console.log(JSON.stringify(result, null, 2));
}

interface UiElement {
  id?: string | number;
  type?: string;
  label?: string;
  value?: string | number | boolean;
  min?: number;
  max?: number;
  options?: string[];
}

interface UiSnapshot {
  screen?: string;
  elements?: UiElement[];
}

function isUiElement(value: unknown): value is UiElement {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;

  if ("id" in v && typeof v.id !== "string" && typeof v.id !== "number") return false;
  if ("type" in v && typeof v.type !== "string") return false;
  if ("label" in v && typeof v.label !== "string") return false;
  if ("min" in v && typeof v.min !== "number") return false;
  if ("max" in v && typeof v.max !== "number") return false;
  if ("options" in v) {
    if (!Array.isArray(v.options)) return false;
    if (!v.options.every((o) => typeof o === "string")) return false;
  }

  return true;
}

function isUiSnapshot(value: unknown): value is UiSnapshot {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;

  if ("screen" in v && typeof v.screen !== "string") return false;
  if ("elements" in v) {
    if (!Array.isArray(v.elements)) return false;
    if (!v.elements.every(isUiElement)) return false;
  }

  return true;
}

async function cmdUiSnapshot(apiBase: string) {
  const result = (await sendAndWait(apiBase, { type: "ui_snapshot" })) as { data?: unknown };
  if (isUiSnapshot(result.data)) {
    const snap = result.data;
    const screen = snap.screen ?? "<unknown>";
    const elements = Array.isArray(snap.elements) ? snap.elements : [];

    console.log(`Screen: ${screen}`);
    console.log(`Elements (${elements.length}):`);

    for (const el of elements) {
      const id = el.id ?? "<no-id>";
      const type = el.type ?? "<unknown>";
      const label = el.label ?? "";

      const parts = [`  ${id}  [${type}]  "${label}"`];

      if (el.type === "slider" || el.type === "int_slider") {
        parts.push(`  value=${String(el.value)}  range=[${String(el.min)}, ${String(el.max)}]`);
      } else if (el.type === "checkbox") {
        parts.push(`  value=${String(el.value)}`);
      } else if (el.type === "combo") {
        const options = Array.isArray(el.options) ? el.options : [];
        parts.push(
          `  value="${String(el.value ?? "")}"  options=[${options.join(", ")}]`,
        );
      }
      console.log(parts.join(""));
    }
  } else {
    console.log(JSON.stringify(result, null, 2));
  }
}

async function cmdUiClick(apiBase: string, flags: Record<string, string>) {
  const element = requireFlag(flags, "element");
  const result = await sendAndWait(apiBase, { type: "ui_click", element_id: element });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdUiSetValue(apiBase: string, flags: Record<string, string>) {
  const element = requireFlag(flags, "element");
  const value = requireFlag(flags, "value");
  const result = await sendAndWait(apiBase, { type: "ui_set_value", element_id: element, value });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdPressKey(apiBase: string, flags: Record<string, string>) {
  const key = requireFlag(flags, "key");
  const valid = ["f1", "escape", "m", "e", "p", "h"];
  if (!valid.includes(key)) die(`--key must be one of: ${valid.join(", ")}`);
  const result = await sendAndWait(apiBase, { type: "press_key", key });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdSetEvolutionOverlay(apiBase: string, flags: Record<string, string>) {
  const mode = requireFlag(flags, "mode");
  const valid = ["off", "wet_preference", "altitude_preference", "abiotic_fitness", "competition_stress", "generation"];
  if (!valid.includes(mode)) die(`--mode must be one of: ${valid.join(", ")}`);
  const result = await sendAndWait(apiBase, { type: "set_evolution_overlay", mode });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdSetPopulationLens(apiBase: string, flags: Record<string, string>) {
  const open = requireFlag(flags, "open");
  if (open !== "true" && open !== "false") die(`--open must be true or false`);
  const result = await sendAndWait(apiBase, { type: "set_population_lens", open: open === "true" });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdInspectEvolutionRegion(apiBase: string, flags: Record<string, string>) {
  const x = requireFloat(flags, "x");
  const z = requireFloat(flags, "z");
  const radius = optionalFloat(flags, "radius") ?? 256;
  const result = await sendAndWait(apiBase, { type: "inspect_evolution_region", x, z, radius });
  console.log(JSON.stringify(result, null, 2));
}

async function cmdMove(apiBase: string, flags: Record<string, string>) {
  const key = requireFlag(flags, "key");
  const valid = ["w", "a", "s", "d", "up", "down"];
  if (!valid.includes(key)) die(`--key must be one of: ${valid.join(", ")}`);

  const duration = optionalFloat(flags, "duration") ?? 200;

  // Press key
  await sendAndWait(apiBase, { type: "set_move_key", key, pressed: true });
  // Hold for duration
  await new Promise((r) => setTimeout(r, duration));
  // Release key
  const result = await sendAndWait(apiBase, { type: "set_move_key", key, pressed: false });
  console.log(JSON.stringify(result, null, 2));
}

// --- Main ---

const USAGE = `Usage: bun tools/debug-cli/cli.ts <command> [options]

Commands:
  state                                    Get current telemetry state
  screenshot                               Capture a screenshot
  save                                     Save the world (camera + plant state)
  set_day_speed   --value <n>              Set day/night cycle speed
  set_time        --hour <0..24)           Set time of day directly, 24 exclusive (e.g. 12=midday, 0=midnight)
  set_camera_position --x <n> --y <n> --z <n>  Teleport camera
  set_camera_look --yaw <n> --pitch <n>    Set camera orientation
  map_right_click --u <0..1> --v <0..1>    Right-click the open world map
  save_favorite   --slot <1..5>            Save current viewpoint to a favorite slot
  recall_favorite --slot <1..5>            Recall a saved favorite viewpoint
  find_nearest    --kind <house|tree|fern>   Find nearest object
  look_at         --id <object_id> [--distance <n>]  Look at object
  move            --key <w|a|s|d|up|down> [--duration <ms>]  Move camera
  press_key       --key <f1|escape|m|e|p|h>  Press a key (toggle config panel, map, evolution overlay, screenshot→clipboard, help, etc.)
  set_evolution_overlay --mode <off|wet_preference|altitude_preference|abiotic_fitness|competition_stress|generation>
                                           Set plant evolution overlay mode
  inspect_evolution_region --x <n> --z <n> [--radius <n>]
  set_population_lens --open <true|false>
                                           Summarize genes/phenotypes around a world position
  ui_snapshot                              Get all interactive UI elements
  ui_click        --element <id>           Click a button or toggle checkbox
  ui_set_value    --element <id> --value <v>  Set slider/combo/checkbox value

Options:
  --api <url>    API base URL (default: ${DEFAULT_API})
  --name <name>  Target a launcher instance by name (resolves its port from
                 instances/<name>/instance.json; overridden by --api)`;

/**
 * Resolves the API base URL for a named instance from its launcher-written
 * registry file (`instances/<name>/instance.json`). Lets callers target a
 * specific running instance without knowing its (OS-assigned) port.
 */
function apiForInstance(name: string): string {
  // Reject names that could traverse out of instances/ (matches the game's
  // validate_instance_name rules) before using one in a filesystem path.
  if (!/^[A-Za-z0-9_-]+$/.test(name)) {
    die(`invalid instance name '${name}' (use letters, digits, '_' or '-')`);
  }
  const file = `instances/${name}/instance.json`;
  let meta: { api?: unknown; pid?: unknown };
  try {
    meta = JSON.parse(readFileSync(file, "utf8"));
  } catch {
    die(`no instance '${name}' found (expected ${file}) — is it running?`);
  }
  // Guard against a corrupt/hand-edited registry: api must be a non-empty string.
  if (typeof meta.api !== "string" || meta.api.length === 0) {
    die(`instance '${name}' has no valid api url in ${file}`);
  }
  // The launcher always records an integer pid; if it's missing or non-integer
  // the registry is corrupt, so fail fast rather than trusting a possibly-stale
  // api. And if the launcher was killed abruptly it couldn't prune the file, so
  // a dead pid means the entry is stale — say so instead of failing later with
  // an opaque connection error.
  if (typeof meta.pid !== "number" || !Number.isInteger(meta.pid)) {
    die(`instance '${name}' registry is missing a valid pid in ${file} — delete it or relaunch`);
  }
  if (!isProcessAlive(meta.pid)) {
    die(
      `instance '${name}' is stale (pid ${meta.pid} not running) — ` +
        `delete ${file} or relaunch with: bun tools/launch.ts --name ${name}`,
    );
  }
  return meta.api;
}

/** Whether a pid is still running (mirrors the launcher's liveness check). */
function isProcessAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    // EPERM means the process exists but we can't signal it — still alive.
    return (err as NodeJS.ErrnoException).code === "EPERM";
  }
}

async function main() {
  const { command, flags } = parseArgs(process.argv);
  const apiBase = flags.api ?? (flags.name ? apiForInstance(flags.name) : DEFAULT_API);

  try {
    switch (command) {
      case "state":
        await cmdState(apiBase);
        break;
      case "screenshot":
        await cmdScreenshot(apiBase);
        break;
      case "save":
        await cmdSave(apiBase);
        break;
      case "set_day_speed":
        await cmdSetDaySpeed(apiBase, flags);
        break;
      case "set_time":
        await cmdSetTime(apiBase, flags);
        break;
      case "set_camera_position":
        await cmdSetCameraPosition(apiBase, flags);
        break;
      case "set_camera_look":
        await cmdSetCameraLook(apiBase, flags);
        break;
      case "map_right_click":
        await cmdMapRightClick(apiBase, flags);
        break;
      case "save_favorite":
        await cmdSaveFavorite(apiBase, flags);
        break;
      case "recall_favorite":
        await cmdRecallFavorite(apiBase, flags);
        break;
      case "find_nearest":
        await cmdFindNearest(apiBase, flags);
        break;
      case "look_at":
        await cmdLookAt(apiBase, flags);
        break;
      case "move":
        await cmdMove(apiBase, flags);
        break;
      case "press_key":
        await cmdPressKey(apiBase, flags);
        break;
      case "set_evolution_overlay":
        await cmdSetEvolutionOverlay(apiBase, flags);
        break;
      case "inspect_evolution_region":
        await cmdInspectEvolutionRegion(apiBase, flags);
        break;
      case "set_population_lens":
        await cmdSetPopulationLens(apiBase, flags);
        break;
      case "ui_snapshot":
        await cmdUiSnapshot(apiBase);
        break;
      case "ui_click":
        await cmdUiClick(apiBase, flags);
        break;
      case "ui_set_value":
        await cmdUiSetValue(apiBase, flags);
        break;
      default:
        if (command) process.stderr.write(`unknown command: ${command}\n\n`);
        console.log(USAGE);
        process.exit(command ? 1 : 0);
    }
  } catch (err) {
    die(err instanceof Error ? err.message : String(err));
  }
}

await main();
