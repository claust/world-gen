#!/usr/bin/env bun

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

async function cmdSetDaySpeed(apiBase: string, flags: Record<string, string>) {
  const value = requireFloat(flags, "value");
  const result = await sendAndWait(apiBase, { type: "set_day_speed", value });
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

async function cmdPressKey(apiBase: string, flags: Record<string, string>) {
  const key = requireFlag(flags, "key");
  const valid = ["f1", "escape"];
  if (!valid.includes(key)) die(`--key must be one of: ${valid.join(", ")}`);
  const result = await sendAndWait(apiBase, { type: "press_key", key });
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
  set_day_speed   --value <n>              Set day/night cycle speed
  set_camera_position --x <n> --y <n> --z <n>  Teleport camera
  set_camera_look --yaw <n> --pitch <n>    Set camera orientation
  find_nearest    --kind <house|tree|fern>   Find nearest object
  look_at         --id <object_id> [--distance <n>]  Look at object
  move            --key <w|a|s|d|up|down> [--duration <ms>]  Move camera
  press_key       --key <f1|escape>      Press a key (toggle config panel, etc.)

Options:
  --api <url>    API base URL (default: ${DEFAULT_API})`;

async function main() {
  const { command, flags } = parseArgs(process.argv);
  const apiBase = flags.api ?? DEFAULT_API;

  try {
    switch (command) {
      case "state":
        await cmdState(apiBase);
        break;
      case "screenshot":
        await cmdScreenshot(apiBase);
        break;
      case "set_day_speed":
        await cmdSetDaySpeed(apiBase, flags);
        break;
      case "set_camera_position":
        await cmdSetCameraPosition(apiBase, flags);
        break;
      case "set_camera_look":
        await cmdSetCameraLook(apiBase, flags);
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
