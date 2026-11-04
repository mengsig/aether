// The service worker: precaches the built shell and serves it offline. It never
// answers daemon traffic (/api, /ws, the /archon mount): those stay live.
//
// A new build's worker waits until every tab of the old build has closed (no skipWaiting, no
// claim), so an open tab keeps the precache its lazy chunks were built against.

interface ExtendableEvent extends Event { waitUntil(promise: Promise<unknown>): void }
interface FetchEvent extends ExtendableEvent { readonly request: Request; respondWith(response: Promise<Response>): void }
interface WorkerScope {
  readonly location: Location;
  readonly registration: { readonly scope: string };
  addEventListener(type: "install" | "activate", listener: (event: ExtendableEvent) => void): void;
  addEventListener(type: "fetch", listener: (event: FetchEvent) => void): void;
}

// The build replaces both placeholders with literals (serviceWorkerPlugin in vite.config.ts).
const PRECACHE = "__AETHER_PRECACHE__" as unknown as readonly string[];
const VERSION = "__AETHER_VERSION__";
if (!Array.isArray(PRECACHE) || VERSION.startsWith("__")) throw new Error("This service worker was not built: it has no precache list");
const CACHE_PREFIX = "aether-web-";
// The retired phone app's worker had the same URL and scope; its shell caches are left to clear.
const LEGACY_CACHE_PREFIX = "aether-shell-";
const CACHE = `${CACHE_PREFIX}${VERSION}`;
const LIVE_PREFIXES = ["/api", "/ws", "/archon"];

const worker = self as unknown as WorkerScope;
const shellUrl = new URL("./", worker.registration.scope).href;
const precached = new Set(PRECACHE.map((path) => new URL(path, worker.registration.scope).href));

function isLive(url: URL): boolean {
  return LIVE_PREFIXES.some((prefix) => url.pathname === prefix || url.pathname.startsWith(`${prefix}/`));
}

worker.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll([...precached])));
});

worker.addEventListener("activate", (event) => {
  // Activation comes only once no tab runs an older build, so its caches have no reader left.
  const stale = (name: string) => (name.startsWith(CACHE_PREFIX) && name !== CACHE) || name.startsWith(LEGACY_CACHE_PREFIX);
  event.waitUntil(caches.keys().then((names) => Promise.all(names.filter(stale).map((name) => caches.delete(name)))));
});

worker.addEventListener("fetch", (event) => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method !== "GET" || url.origin !== worker.location.origin || isLive(url)) return;
  if (request.mode === "navigate") event.respondWith(networkFirst(request));
  else if (precached.has(url.href)) event.respondWith(cacheFirst(request));
});

/** Pages come from the network while it answers, so a new build shows at once; offline, the cached shell. */
async function networkFirst(request: Request): Promise<Response> {
  try {
    return await fetch(request);
  } catch (error) {
    const cached = await caches.match(shellUrl, { cacheName: CACHE });
    if (cached) return cached;
    throw error;
  }
}

/** Built assets are content-hashed, so a cached copy is always current. */
async function cacheFirst(request: Request): Promise<Response> {
  const cached = await caches.match(request, { cacheName: CACHE });
  return cached ?? fetch(request);
}
