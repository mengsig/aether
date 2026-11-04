// Where the desktop app finds the shared frontend and the daemon schemas: the committed
// web/dist bundle the listener embeds, and the generated protocol schemas.

import path from "node:path";

export const DESKTOP_ROOT = path.resolve(import.meta.dirname, "..", "..");
export const WEB_ROOT = path.resolve(DESKTOP_ROOT, "..", "..");
export const REPO_ROOT = path.resolve(WEB_ROOT, "..");

// A packaged app runs from resources/app.asar; electron-builder.yml copies web/dist and the two
// schemas, unchanged, into resources/ beside it.
export const PACKAGED_ASAR = "app.asar";
const PACKAGED = path.basename(DESKTOP_ROOT) === PACKAGED_ASAR;
const RESOURCES = path.dirname(DESKTOP_ROOT);

/** The very files `crates/aether-remote` compiles into the listener. */
export const WEB_DIST = PACKAGED ? path.join(RESOURCES, "web", "dist") : path.join(WEB_ROOT, "dist");
export const AETHER_SCHEMA = PACKAGED
  ? path.join(RESOURCES, "schemas", "protocol.schema.json")
  : path.join(REPO_ROOT, "clients", "protocol.schema.json");
export const ARCHON_SCHEMA = PACKAGED
  ? path.join(RESOURCES, "schemas", "archon.protocol.schema.json")
  : path.join(WEB_ROOT, "schemas", "archon.protocol.schema.json");
export const PRELOAD = path.join(DESKTOP_ROOT, "src", "preload", "index.cjs");
