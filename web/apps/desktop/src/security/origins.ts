// The `remote.origins` rules of CONTRACTS.md §2.4: exact lowercase `https://host[:port]`
// origins, no path, query, fragment or userinfo, and one session partition per origin.

import { createHash } from "node:crypto";
import { MAX_REMOTE_ORIGINS, MAX_STRING_BYTES, utf8Bytes } from "./limits.ts";

/** Why a value is not a remote origin; the renderer sees the message. */
export class OriginError extends Error {
  override readonly name = "OriginError";
}

function parse(value: string): URL {
  try {
    return new URL(value);
  } catch (error) {
    throw new OriginError(`Not an origin: ${JSON.stringify(value.slice(0, 80))}`, { cause: error });
  }
}

// The raw text of a bare origin: scheme, an authority with no userinfo, at most a trailing slash.
const BARE_ORIGIN = /^https:\/\/[^/?#\\@\s]+\/?$/i;

/**
 * The normalized origin `value` names. The URL parser lowercases the host,
 * turns an IDN into punycode and drops a default port; anything beyond the
 * origin is refused, never silently cut away.
 */
export function normalizeOrigin(value: unknown): string {
  if (typeof value !== "string" || value.length === 0) throw new OriginError("An origin is a non-empty string");
  if (utf8Bytes(value) > MAX_STRING_BYTES) throw new OriginError("The origin is too long");
  const url = parse(value);
  if (url.protocol !== "https:") throw new OriginError(`Remote origins use https:, not ${url.protocol}`);
  if (url.username !== "" || url.password !== "" || value.includes("@")) {
    throw new OriginError("A remote origin has no user name or password");
  }
  if (!BARE_ORIGIN.test(value) || url.pathname !== "/" || url.search !== "" || url.hash !== "") {
    throw new OriginError("A remote origin has no path, query or fragment");
  }
  return url.origin;
}

/** The normalized, duplicate-free list a `remote.origins` preference holds. */
export function normalizeOriginList(value: unknown): string[] {
  if (!Array.isArray(value)) throw new OriginError("remote.origins is a list of origins");
  if (value.length > MAX_REMOTE_ORIGINS) throw new OriginError(`remote.origins holds at most ${MAX_REMOTE_ORIGINS} origins`);
  const origins = value.map(normalizeOrigin);
  const unique = [...new Set(origins)];
  if (unique.length !== origins.length) throw new OriginError("remote.origins lists an origin twice");
  return unique;
}

/** The persistent session partition of one remote origin; never shared with app:// or another origin. */
export function remotePartition(origin: string): string {
  if (normalizeOrigin(origin) !== origin) throw new OriginError(`Not a normalized origin: ${origin}`);
  return `persist:remote:${createHash("sha256").update(origin).digest("hex")}`;
}
