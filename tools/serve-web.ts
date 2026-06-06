// Minimal static file server for the local `web/` build, used to verify the
// wasm base-world download end-to-end. Serves correct MIME types for .wasm and
// .bin and disables caching so reloads always hit fresh bytes.
import { join, normalize } from "node:path";

const ROOT = join(import.meta.dir, "..", "web");
const PORT = Number(process.env.PORT ?? 8787);

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".bin": "application/octet-stream",
  ".json": "application/json",
  ".jpg": "image/jpeg",
  ".png": "image/png",
};

const server = Bun.serve({
  port: PORT,
  async fetch(req) {
    let path: string;
    try {
      path = decodeURIComponent(new URL(req.url).pathname);
    } catch {
      // Malformed percent-encoding in the path — reject rather than throw.
      return new Response("bad request", { status: 400 });
    }
    if (path === "/") path = "/index.html";
    const full = normalize(join(ROOT, path));
    if (!full.startsWith(ROOT)) return new Response("forbidden", { status: 403 });
    const file = Bun.file(full);
    if (!(await file.exists())) return new Response("not found", { status: 404 });
    const dot = full.lastIndexOf(".");
    const slash = full.lastIndexOf("/");
    // Only treat a trailing ".xxx" as an extension if the dot is in the final
    // path segment; otherwise (no dot, or dot only in a parent dir) use the
    // octet-stream fallback instead of misreading the last character.
    const ext = dot > slash ? full.slice(dot) : "";
    return new Response(file, {
      headers: {
        "content-type": MIME[ext] ?? "application/octet-stream",
        "cache-control": "no-store",
      },
    });
  },
});

console.log(`[serve-web] serving ${ROOT} at http://localhost:${server.port}`);
