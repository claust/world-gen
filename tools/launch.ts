#!/usr/bin/env bun
/**
 * Multi-instance launcher.
 *
 *   bun tools/launch.ts                    # build + launch a uniquely-named instance
 *   bun tools/launch.ts --name alpha       # launch the named instance "alpha"
 *   bun tools/launch.ts --no-build         # skip cargo build (reuse existing binary)
 *   bun tools/launch.ts -- --benchmark x   # pass extra args through to the game
 *   bun tools/launch.ts list               # list live instances (prunes dead ones)
 *
 * Why this exists: the game can't run twice on one machine because the debug
 * API port (default 7777) and the on-disk state (save/config/plants/captures)
 * collide. This launcher fixes both:
 *
 *   - Port: it starts the game with `--debug-api-bind 127.0.0.1:0`, so the OS
 *     hands out a guaranteed-free port (no scan, no TOCTOU race), then reads the
 *     real port back from the game's own startup log.
 *   - State: it passes `--instance-name <name>`, so the game roots everything
 *     under `instances/<name>/`. Shared assets (herbarium/config) are seeded
 *     from the repo root on first launch so a new instance isn't a blank slate.
 *
 * Discovery: each running instance is recorded at `instances/<name>/instance.json`
 * with its name, pid, port, and dirs. Other tools (and other LLMs) read those to
 * find what's live — e.g. `bun tools/debug-cli/cli.ts state --name alpha`.
 */
import {
  mkdirSync,
  writeFileSync,
  rmSync,
  existsSync,
  readdirSync,
  readFileSync,
  copyFileSync,
} from "node:fs";
import { join } from "node:path";

const INSTANCES_ROOT = "instances";
// Cargo appends .exe on Windows; match it so the launcher finds the binary there.
const BIN =
  process.platform === "win32"
    ? "target/release/FloraForge.exe"
    : "target/release/FloraForge";
// Assets seeded into a fresh instance dir so it starts from the repo's current
// tuned state instead of the game's built-in defaults. Keys match storage keys.
const SEED_ASSETS = ["herbarium.json", "config.json"];
const PORT_LINE = /debug api listening on [\d.]+:(\d+)/;
const WORLD_GEN_LOG_TARGET = "world_gen";

// --- discovery helpers ---

function metaPath(name: string): string {
  return join(INSTANCES_ROOT, name, "instance.json");
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    // EPERM means the process exists but we can't signal it — still alive.
    return (err as NodeJS.ErrnoException).code === "EPERM";
  }
}

type InstanceMeta = {
  name: string;
  pid: number;
  port: number;
  api: string;
  instanceDir: string;
  capturesDir: string;
  startedAt: string;
};

function listInstances(): Array<InstanceMeta & { alive: boolean }> {
  if (!existsSync(INSTANCES_ROOT)) return [];
  const out: Array<InstanceMeta & { alive: boolean }> = [];
  for (const name of readdirSync(INSTANCES_ROOT)) {
    const file = metaPath(name);
    if (!existsSync(file)) continue;
    let meta: InstanceMeta;
    try {
      meta = JSON.parse(readFileSync(file, "utf8"));
    } catch {
      rmSync(file, { force: true, recursive: true });
      continue;
    }
    // The file is on disk and may be corrupted or hand-edited; a non-numeric
    // pid would make isAlive() throw and crash `list`/`launch`. Treat any
    // record without a valid pid as a dead/stale entry and prune it.
    if (typeof meta.pid !== "number" || !Number.isInteger(meta.pid)) {
      rmSync(file, { force: true, recursive: true });
      continue;
    }
    const alive = isAlive(meta.pid);
    if (!alive) {
      // Stale record from a crashed/closed instance — prune it. The state dir
      // (save/config/captures) is left intact so the world can be resumed.
      rmSync(file, { force: true, recursive: true });
      continue;
    }
    out.push({ ...meta, alive });
  }
  return out;
}

function cmdList(): void {
  const instances = listInstances();
  for (const i of instances) {
    console.log(`${i.name}\tpid=${i.pid}\t${i.api}\t${i.instanceDir}`);
  }
  if (instances.length === 0) console.log("no live instances");
  // Always emit machine-readable JSON (an empty array when none) as the final
  // line, so programmatic callers can parse it unconditionally.
  console.log(JSON.stringify(instances));
}

// --- launch ---

function uniqueName(): string {
  // Short, sortable, collision-resistant without needing to read the dir.
  return `inst-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

function seedAssets(instanceDir: string): void {
  for (const asset of SEED_ASSETS) {
    const dest = join(instanceDir, asset);
    if (existsSync(asset) && !existsSync(dest)) {
      copyFileSync(asset, dest);
    }
  }
}

function launcherEnv(): Record<string, string | undefined> {
  const env = { ...process.env };
  const rustLog = env.RUST_LOG;
  const hasWorldGenTarget =
    rustLog
      ?.split(",")
      .some((part) => part.trim().startsWith(`${WORLD_GEN_LOG_TARGET}=`)) ?? false;

  if (!hasWorldGenTarget) {
    env.RUST_LOG = rustLog ? `${rustLog},${WORLD_GEN_LOG_TARGET}=info` : `${WORLD_GEN_LOG_TARGET}=info`;
  }

  return env;
}

/** Tees a process stream to our own stdio while scanning each line. */
async function tee(
  stream: ReadableStream<Uint8Array>,
  sink: NodeJS.WriteStream,
  onLine: (line: string) => void,
): Promise<void> {
  const decoder = new TextDecoder();
  let buf = "";
  for await (const chunk of stream) {
    sink.write(chunk);
    buf += decoder.decode(chunk, { stream: true });
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      onLine(buf.slice(0, nl));
      buf = buf.slice(nl + 1);
    }
  }
  // Flush any bytes the decoder held back (an incomplete multibyte sequence at
  // a chunk boundary) so the final line isn't truncated.
  buf += decoder.decode();
  if (buf) onLine(buf);
}

async function cmdLaunch(name: string, noBuild: boolean, extra: string[]): Promise<number> {
  const existing = listInstances().find((i) => i.name === name);
  if (existing) {
    console.error(
      `instance '${name}' is already running (pid ${existing.pid}, ${existing.api})`,
    );
    return 1;
  }

  if (!noBuild) {
    console.log("> cargo build --release");
    const build = Bun.spawnSync(["cargo", "build", "--release"], { stdio: ["inherit", "inherit", "inherit"] });
    if (build.exitCode !== 0) {
      console.error("build failed");
      return build.exitCode ?? 1;
    }
  }
  if (!existsSync(BIN)) {
    console.error(`binary not found at ${BIN} (run without --no-build first)`);
    return 1;
  }

  const instanceDir = join(INSTANCES_ROOT, name);
  const capturesDir = join(instanceDir, "captures");
  mkdirSync(instanceDir, { recursive: true });
  seedAssets(instanceDir);

  const proc = Bun.spawn({
    cmd: [BIN, "--debug-api-bind", "127.0.0.1:0", "--instance-name", name, ...extra],
    stdout: "pipe",
    stderr: "pipe",
    env: launcherEnv(),
  });

  let resolvePort!: (port: number) => void;
  const portFound = new Promise<number>((resolve) => {
    resolvePort = resolve;
  });
  const onLine = (line: string) => {
    const m = line.match(PORT_LINE);
    if (m) resolvePort(Number(m[1]));
  };

  // env_logger writes to stderr; tee both so the user sees everything.
  const teeing = Promise.all([
    tee(proc.stdout, process.stdout, onLine),
    tee(proc.stderr, process.stderr, onLine),
  ]);

  // Forward Ctrl-C / termination to the child so it shuts down cleanly.
  const forward = (sig: NodeJS.Signals) => proc.kill(sig === "SIGINT" ? 2 : 15);
  process.on("SIGINT", () => forward("SIGINT"));
  process.on("SIGTERM", () => forward("SIGTERM"));

  // Race the port log against early exit (crash/bind failure) and a timeout in
  // case the log format ever changes and the port line never arrives.
  const PORT_TIMEOUT_MS = 15_000;
  let timer: ReturnType<typeof setTimeout>;
  const portTimeout = new Promise<number>((resolve) => {
    timer = setTimeout(() => resolve(-1), PORT_TIMEOUT_MS);
  });
  const port = await Promise.race([
    portFound,
    proc.exited.then(() => -1),
    portTimeout,
  ]);
  clearTimeout(timer!);

  if (port < 0) {
    // Either the game already exited, or it's still running but never logged
    // the port (timeout). Kill it so it doesn't linger as an orphan holding a
    // port with no registry entry — and so the streams close and `teeing`
    // below can resolve instead of hanging on the still-open pipes.
    proc.kill(); // SIGTERM
    // Escalate to SIGKILL if it ignores SIGTERM, and bound the pipe wait so a
    // child that never closes its streams can't hang the launcher forever.
    const sigkill = setTimeout(() => {
      try {
        proc.kill(9);
      } catch {
        /* already gone */
      }
    }, 3000);
    await Promise.race([
      teeing,
      new Promise<void>((resolve) => setTimeout(resolve, 5000)),
    ]);
    clearTimeout(sigkill);
    console.error(
      "failed to detect debug API port from startup logs (game exited, or its logging format changed)",
    );
    return proc.exitCode ?? 1;
  }

  const api = `http://127.0.0.1:${port}`;
  const meta: InstanceMeta = {
    name,
    pid: proc.pid,
    port,
    api,
    instanceDir,
    capturesDir,
    startedAt: new Date().toISOString(),
  };
  // Don't let a write failure (permissions, disk full) crash the launcher and
  // orphan the running game: warn, note that discovery won't work for it, and
  // keep managing the child below (signal forwarding + cleanup still apply).
  try {
    writeFileSync(metaPath(name), JSON.stringify(meta, null, 2));
  } catch (err) {
    console.error(
      `warning: failed to write registry ${metaPath(name)}: ${(err as Error).message}\n` +
        `  the game is running on ${api} but won't be discoverable by --name`,
    );
  }

  // Machine-readable handshake line so a calling LLM/tool learns the port.
  console.log(`[launch] ready ${JSON.stringify(meta)}`);
  console.log(
    `[launch] '${name}' on ${api} — e.g. bun tools/debug-cli/cli.ts state --name ${name}`,
  );

  await proc.exited;
  await teeing;
  // Instance is gone — drop the runtime record (keep the state dir).
  rmSync(metaPath(name), { force: true, recursive: true });
  return proc.exitCode ?? 0;
}

// --- entry ---

async function main(): Promise<void> {
  const argv = process.argv.slice(2);

  if (argv[0] === "list") {
    cmdList();
    return;
  }

  const dashDash = argv.indexOf("--");
  const head = dashDash >= 0 ? argv.slice(0, dashDash) : argv;
  const extra = dashDash >= 0 ? argv.slice(dashDash + 1) : [];

  let name = "";
  let noBuild = false;
  for (let i = 0; i < head.length; i++) {
    if (head[i] === "--name") {
      const value = head[++i];
      // Require a real value — don't silently swallow the next flag (e.g.
      // `--name --no-build`) or fall through to a random name on a bare --name.
      if (value === undefined || value.startsWith("--")) {
        console.error("--name requires a value");
        process.exit(1);
      }
      name = value;
    } else if (head[i] === "--no-build") noBuild = true;
    else {
      console.error(`unknown argument: ${head[i]}`);
      process.exit(1);
    }
  }
  if (!name) name = uniqueName();
  if (!/^[A-Za-z0-9_-]+$/.test(name)) {
    console.error(`invalid instance name '${name}' (use letters, digits, '_' or '-')`);
    process.exit(1);
  }

  const code = await cmdLaunch(name, noBuild, extra);
  process.exit(code);
}

main();
