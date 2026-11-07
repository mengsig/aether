// The app:// scheme: serves web/dist byte for byte, with the CSP on every response. Paths
// follow the listener's rule (crates/aether-remote selection.rs): URL-safe segments, no dot files.

import { readFile } from "node:fs/promises";
import path from "node:path";
import { APP_HEADERS, APP_HOST, APP_SCHEME } from "../security/csp.ts";

const SEGMENT = /^[A-Za-z0-9_~-][A-Za-z0-9._~-]*$/;

// The listener's content types (crates/aether-remote/src/assets.rs).
const CONTENT_TYPES: Readonly<Record<string, string>> = {
  html: "text/html; charset=utf-8", js: "text/javascript; charset=utf-8", mjs: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8", json: "application/json", map: "application/json",
  webmanifest: "application/manifest+json", svg: "image/svg+xml", png: "image/png", jpg: "image/jpeg",
  jpeg: "image/jpeg", gif: "image/gif", ico: "image/x-icon", webp: "image/webp", woff2: "font/woff2",
  woff: "font/woff", txt: "text/plain; charset=utf-8", wasm: "application/wasm",
};

export function contentType(file: string): string {
  const extension = file.includes(".") ? file.slice(file.lastIndexOf(".") + 1) : "";
  return CONTENT_TYPES[extension] ?? "application/octet-stream";
}

/** The bundle file `url` names, or undefined when it names none the bundle could hold. */
export function bundleFile(distDir: string, url: string): string | undefined {
  const parsed = URL.parse(url);
  if (parsed === null || parsed.protocol !== `${APP_SCHEME}:` || parsed.host !== APP_HOST) return undefined;
  const pathname = parsed.pathname === "/" ? "/index.html" : parsed.pathname;
  const segments = pathname.slice(1).split("/");
  if (!segments.every((segment) => SEGMENT.test(segment))) return undefined;
  const file = path.join(distDir, ...segments);
  if (!file.startsWith(distDir + path.sep)) throw new Error(`${url} resolved outside the bundle`);
  return file;
}

function notFound(): Response {
  return new Response("Not found", { status: 404, headers: { ...APP_HEADERS, "content-type": "text/plain; charset=utf-8" } });
}

/** The app:// response for `request`: the file's exact bytes, or 404. Only GET reads. */
export async function serveApp(distDir: string, request: Request): Promise<Response> {
  if (!path.isAbsolute(distDir)) throw new Error("the bundle directory is absolute");
  if (request.method !== "GET" && request.method !== "HEAD") return new Response(null, { status: 405, headers: APP_HEADERS });
  const file = bundleFile(distDir, request.url);
  if (file === undefined) return notFound();
  let body: Buffer;
  try {
    body = await readFile(file);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT" || (error as NodeJS.ErrnoException).code === "EISDIR") return notFound();
    throw new Error(`Reading ${file} for ${request.url}`, { cause: error });
  }
  const headers = { ...APP_HEADERS, "content-type": contentType(file) };
  return new Response(request.method === "HEAD" ? null : new Uint8Array(body), { status: 200, headers });
}
