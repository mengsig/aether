// Where the desktop app finds the shared frontend and the daemon schemas: the committed
// web/dist bundle the listener embeds, and the generated protocol schemas.

import path from "node:path";

export const DESKTOP_ROOT = path.resolve(import.meta.dirname, "..", "..");
export const WEB_ROOT = path.resolve(DESKTOP_ROOT, "..", "..");
export const REPO_ROOT = path.resolve(WEB_ROOT, "..");

/** The very files `crates/aether-remote` compiles into the listener. */
export const WEB_DIST = path.join(WEB_ROOT, "dist");
export const AETHER_SCHEMA = path.join(REPO_ROOT, "clients", "protocol.schema.json");
export const ARCHON_SCHEMA = path.join(WEB_ROOT, "schemas", "archon.protocol.schema.json");
export const PRELOAD = path.join(DESKTOP_ROOT, "src", "preload", "index.cjs");
