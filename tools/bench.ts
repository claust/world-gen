#!/usr/bin/env bun
/**
 * One-command FPS benchmark runner.
 *
 *   bun tools/bench.ts                       # build + run default flythrough
 *   bun tools/bench.ts benchmarks/foo.json   # use a specific script
 *   bun tools/bench.ts --no-build            # skip cargo build (use existing binary)
 *   bun tools/bench.ts --baseline            # save this run as the baseline
 *
 * The game launches in `--benchmark` mode, replays a deterministic flythrough,
 * writes benchmarks/latest.json, and exits. This wrapper prints the report and,
 * if a baseline exists, the delta against it.
 */
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, copyFileSync } from "node:fs";
import { dirname, join } from "node:path";

const args = process.argv.slice(2);
const noBuild = args.includes("--no-build");
const saveBaseline = args.includes("--baseline");
const scriptPath =
  args.find((a) => !a.startsWith("--")) ?? "benchmarks/flythrough.json";
const reportPath = join(dirname(scriptPath), "latest.json");
const baselinePath = join(dirname(scriptPath), "baseline.json");

// Threshold (%) below which a drop is treated as a regression for the exit code.
const REGRESSION_PCT = 5;

function run(cmd: string, cmdArgs: string[]): number {
  console.log(`> ${cmd} ${cmdArgs.join(" ")}`);
  const res = spawnSync(cmd, cmdArgs, { stdio: "inherit" });
  return res.status ?? 1;
}

if (!noBuild) {
  const code = run("cargo", ["build", "--release"]);
  if (code !== 0) {
    console.error("build failed");
    process.exit(code);
  }
}

const runCode = run("cargo", [
  "run",
  "--release",
  "--",
  "--benchmark",
  scriptPath,
]);
if (runCode !== 0) {
  console.error("benchmark run failed");
  process.exit(runCode);
}

if (!existsSync(reportPath)) {
  console.error(`no report produced at ${reportPath}`);
  process.exit(1);
}

const report = JSON.parse(readFileSync(reportPath, "utf8"));

console.log("\n──────── FPS BENCHMARK ────────");
console.log(`script        : ${scriptPath}`);
console.log(`frames        : ${report.frames}`);
console.log(`average FPS   : ${report.avg_fps.toFixed(1)}`);
console.log(`1% low FPS    : ${report.one_percent_low_fps.toFixed(1)}`);
console.log(`min / max FPS : ${report.min_fps.toFixed(1)} / ${report.max_fps.toFixed(1)}`);
console.log(
  `frame ms (mean/p95/p99) : ${report.mean_frame_ms.toFixed(2)} / ${report.p95_frame_ms.toFixed(2)} / ${report.p99_frame_ms.toFixed(2)}`,
);

let exitCode = 0;

if (existsSync(baselinePath) && !saveBaseline) {
  const base = JSON.parse(readFileSync(baselinePath, "utf8"));
  const pct = (cur: number, ref: number) => ((cur - ref) / ref) * 100;
  const avgDelta = pct(report.avg_fps, base.avg_fps);
  const lowDelta = pct(report.one_percent_low_fps, base.one_percent_low_fps);

  console.log("\n──────── vs BASELINE ────────");
  console.log(`avg FPS  : ${base.avg_fps.toFixed(1)} → ${report.avg_fps.toFixed(1)} (${avgDelta >= 0 ? "+" : ""}${avgDelta.toFixed(1)}%)`);
  console.log(`1% low   : ${base.one_percent_low_fps.toFixed(1)} → ${report.one_percent_low_fps.toFixed(1)} (${lowDelta >= 0 ? "+" : ""}${lowDelta.toFixed(1)}%)`);

  if (avgDelta < -REGRESSION_PCT || lowDelta < -REGRESSION_PCT) {
    console.error(`\n⚠️  regression: FPS dropped more than ${REGRESSION_PCT}%`);
    exitCode = 1;
  }
}

if (saveBaseline) {
  copyFileSync(reportPath, baselinePath);
  console.log(`\nsaved baseline → ${baselinePath}`);
}

process.exit(exitCode);
