#!/usr/bin/env bun
/**
 * CDP (Chrome DevTools Protocol) script to test WASM build in headless Chrome.
 * Connects via WebSocket, navigates to the local server, collects console logs,
 * and takes a screenshot.
 */

import { writeFileSync } from "fs";

const WS_URL = process.argv[2];
if (!WS_URL) {
  console.error("Usage: bun cdp-wasm-test.ts <websocket-url>");
  process.exit(1);
}

const SCREENSHOT_PATH = "/Users/claus/Repos/world-gen/captures/wasm-test.png";
const TARGET_URL = "http://localhost:8080";
const GPU_INIT_WAIT_MS = 10000;

let msgId = 0;
const pending = new Map<number, { resolve: (v: any) => void; reject: (e: any) => void }>();
const consoleLogs: { level: string; text: string }[] = [];
const exceptions: string[] = [];
let pageLoaded = false;
let pageLoadResolve: (() => void) | null = null;

const ws = new WebSocket(WS_URL);

ws.addEventListener("open", async () => {
  console.log("[CDP] Connected to Chrome DevTools Protocol");
  try {
    await runTest();
  } catch (err) {
    console.error("[CDP] Test failed:", err);
    process.exit(1);
  }
});

ws.addEventListener("message", (event) => {
  const msg = JSON.parse(String(event.data));

  // Handle responses to our commands
  if (msg.id !== undefined && pending.has(msg.id)) {
    const p = pending.get(msg.id)!;
    pending.delete(msg.id);
    if (msg.error) {
      p.reject(new Error(`CDP error: ${JSON.stringify(msg.error)}`));
    } else {
      p.resolve(msg.result);
    }
    return;
  }

  // Handle events
  if (msg.method === "Runtime.consoleAPICalled") {
    const level = msg.params.type;
    const text = msg.params.args.map((a: any) => a.value ?? a.description ?? JSON.stringify(a)).join(" ");
    consoleLogs.push({ level, text });
    // Print errors/warnings immediately
    if (level === "error" || level === "warning") {
      console.log(`[Console ${level}] ${text}`);
    }
  }

  if (msg.method === "Runtime.exceptionThrown") {
    const detail = msg.params.exceptionDetails;
    const text = detail.text + (detail.exception?.description ? `: ${detail.exception.description}` : "");
    exceptions.push(text);
    console.log(`[Exception] ${text}`);
  }

  if (msg.method === "Page.loadEventFired") {
    pageLoaded = true;
    if (pageLoadResolve) pageLoadResolve();
  }
});

ws.addEventListener("error", (event) => {
  console.error("[CDP] WebSocket error:", event);
  process.exit(1);
});

ws.addEventListener("close", () => {
  console.log("[CDP] WebSocket closed");
});

function send(method: string, params: any = {}): Promise<any> {
  const id = ++msgId;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    ws.send(JSON.stringify({ id, method, params }));
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

async function runTest() {
  // 1. Enable domains
  console.log("[CDP] Enabling Runtime, Page, and Log domains...");
  await send("Runtime.enable");
  await send("Page.enable");
  await send("Log.enable");

  // 2. Navigate to the target URL
  console.log(`[CDP] Navigating to ${TARGET_URL}...`);
  const navResult = await send("Page.navigate", { url: TARGET_URL });
  console.log(`[CDP] Navigation initiated, frameId: ${navResult.frameId}`);

  // Wait for page load event
  if (!pageLoaded) {
    console.log("[CDP] Waiting for page load event...");
    await new Promise<void>((resolve) => {
      pageLoadResolve = resolve;
      setTimeout(() => {
        if (!pageLoaded) {
          console.log("[CDP] Page load timeout after 15s, continuing anyway...");
          resolve();
        }
      }, 15000);
    });
  }
  console.log("[CDP] Page loaded.");

  // 3. Wait for GPU initialization and rendering
  console.log(`[CDP] Waiting ${GPU_INIT_WAIT_MS / 1000}s for GPU init and rendering...`);
  await sleep(GPU_INIT_WAIT_MS);

  // 4. Check WebGPU availability
  console.log("[CDP] Checking WebGPU availability...");
  try {
    const gpuCheck = await send("Runtime.evaluate", {
      expression: `(async () => {
        const result = {};
        result.hasNavigatorGPU = !!navigator.gpu;
        if (navigator.gpu) {
          try {
            const adapter = await navigator.gpu.requestAdapter();
            result.hasAdapter = !!adapter;
            if (adapter) {
              const info = adapter.info || {};
              result.adapterInfo = {
                vendor: info.vendor || 'unknown',
                architecture: info.architecture || 'unknown',
                device: info.device || 'unknown',
                description: info.description || 'unknown',
              };
            }
          } catch (e) {
            result.adapterError = e.message;
          }
        }
        return JSON.stringify(result);
      })()`,
      awaitPromise: true,
      returnByValue: true,
    });
    if (gpuCheck.result?.value) {
      const info = JSON.parse(gpuCheck.result.value);
      console.log("[CDP] WebGPU info:", JSON.stringify(info, null, 2));
    }
  } catch (e) {
    console.log("[CDP] Could not check WebGPU:", e);
  }

  // 5. Check canvas state and try to extract canvas content
  console.log("[CDP] Checking canvas state...");
  try {
    const canvasInfo = await send("Runtime.evaluate", {
      expression: `(() => {
        const c = document.getElementById('world-gen-canvas');
        if (!c) return JSON.stringify({ error: 'no canvas found' });
        return JSON.stringify({
          width: c.width,
          height: c.height,
          clientWidth: c.clientWidth,
          clientHeight: c.clientHeight,
          contextType: c.getContext ? 'has getContext' : 'no getContext',
        });
      })()`,
      returnByValue: true,
    });
    if (canvasInfo.result?.value) {
      console.log("[CDP] Canvas info:", canvasInfo.result.value);
    }
  } catch (e) {
    console.log("[CDP] Could not check canvas:", e);
  }

  // 6. Trigger a few animation frames to ensure rendering happens
  console.log("[CDP] Triggering animation frames...");
  try {
    await send("Runtime.evaluate", {
      expression: `new Promise(resolve => {
        let count = 0;
        function raf() {
          count++;
          if (count < 10) {
            requestAnimationFrame(raf);
          } else {
            resolve('fired ' + count + ' rAF callbacks');
          }
        }
        requestAnimationFrame(raf);
      })`,
      awaitPromise: true,
      returnByValue: true,
    });
    console.log("[CDP] Animation frames triggered, waiting 2s more...");
    await sleep(2000);
  } catch (e) {
    console.log("[CDP] rAF check error:", e);
  }

  // 7. Take screenshot via CDP (captures composited output including WebGPU)
  console.log("[CDP] Taking CDP screenshot...");
  const screenshotResult = await send("Page.captureScreenshot", {
    format: "png",
    quality: 100,
    captureBeyondViewport: false,
    optimizeForSpeed: false,
  });

  const buffer = Buffer.from(screenshotResult.data, "base64");
  writeFileSync(SCREENSHOT_PATH, buffer);
  console.log(`[CDP] Screenshot saved to ${SCREENSHOT_PATH} (${buffer.length} bytes)`);

  // 8. Also try to get canvas pixel data directly
  console.log("[CDP] Attempting direct canvas pixel readback...");
  try {
    const canvasData = await send("Runtime.evaluate", {
      expression: `(() => {
        const c = document.getElementById('world-gen-canvas');
        if (!c) return JSON.stringify({ error: 'no canvas' });

        // Try to get a 2d context snapshot -- won't work if webgpu has the context
        // Instead, check if wgpu has configured the canvas
        const ctx = c.getContext('webgpu');
        if (ctx) {
          return JSON.stringify({ contextType: 'webgpu', note: 'Canvas has WebGPU context - CDP screenshot should capture it' });
        }

        // Fallback: try 2d
        try {
          const ctx2d = c.getContext('2d');
          if (ctx2d) {
            const imageData = ctx2d.getImageData(0, 0, Math.min(c.width, 10), Math.min(c.height, 10));
            const nonZero = imageData.data.some(v => v !== 0);
            return JSON.stringify({ contextType: '2d', hasPixels: nonZero });
          }
        } catch(e) {
          return JSON.stringify({ error: e.message });
        }
        return JSON.stringify({ note: 'could not determine context' });
      })()`,
      returnByValue: true,
    });
    if (canvasData.result?.value) {
      console.log("[CDP] Canvas readback:", canvasData.result.value);
    }
  } catch (e) {
    console.log("[CDP] Canvas readback error:", e);
  }

  // 9. Check the DOM for any visible egui elements or error overlays
  console.log("[CDP] Checking DOM for egui/error elements...");
  try {
    const domCheck = await send("Runtime.evaluate", {
      expression: `(() => {
        const body = document.body;
        const allElements = body.querySelectorAll('*');
        const visible = [];
        for (const el of allElements) {
          if (el.tagName !== 'SCRIPT' && el.tagName !== 'CANVAS') {
            const rect = el.getBoundingClientRect();
            if (rect.width > 0 && rect.height > 0) {
              visible.push({ tag: el.tagName, id: el.id, class: el.className, text: el.textContent?.substring(0, 100) });
            }
          }
        }
        return JSON.stringify({ visibleElements: visible.length, elements: visible.slice(0, 10) });
      })()`,
      returnByValue: true,
    });
    if (domCheck.result?.value) {
      console.log("[CDP] DOM check:", domCheck.result.value);
    }
  } catch (e) {
    console.log("[CDP] DOM check error:", e);
  }

  // 10. Report results
  console.log("\n========== RESULTS ==========");
  console.log(`Total console messages: ${consoleLogs.length}`);

  const errors = consoleLogs.filter((l) => l.level === "error");
  const warnings = consoleLogs.filter((l) => l.level === "warning");
  const infos = consoleLogs.filter((l) => l.level === "log" || l.level === "info" || l.level === "debug");

  if (errors.length > 0) {
    console.log(`\n--- ERRORS (${errors.length}) ---`);
    errors.forEach((e) => console.log(`  [error] ${e.text}`));
  } else {
    console.log("\nNo console errors found.");
  }

  if (warnings.length > 0) {
    console.log(`\n--- WARNINGS (${warnings.length}) ---`);
    warnings.forEach((w) => console.log(`  [warn] ${w.text}`));
  }

  if (exceptions.length > 0) {
    console.log(`\n--- EXCEPTIONS (${exceptions.length}) ---`);
    exceptions.forEach((e) => console.log(`  [exception] ${e}`));
  } else {
    console.log("No uncaught exceptions.");
  }

  if (infos.length > 0) {
    console.log(`\n--- INFO/LOG (${infos.length}) ---`);
    infos.forEach((i) => console.log(`  [log] ${i.text}`));
  }

  console.log("\n=============================");

  // Clean up
  ws.close();
  process.exit(errors.length > 0 || exceptions.length > 0 ? 1 : 0);
}
