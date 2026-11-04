// The pairing flow behind `#/pair/:daemon`: Aether enrolls this browser's key
// and waits for the host's confirmation; Archon claims a code for its cookie.

import { DaemonError } from "./errors.ts";
import { errorText, invariant, object, string, type Json } from "./guards.ts";
import { base64url, newIdentity, type IdentityStore } from "./transports/identity.ts";
import { ARCHON_MOUNT } from "./transports/ws-archon.ts";

export type PairingState =
  | { readonly phase: "idle" }
  | { readonly phase: "claiming" }
  | { readonly phase: "confirm"; readonly phrase: string; readonly fingerprint: string }
  | { readonly phase: "paired"; readonly device: Json }
  | { readonly phase: "failed"; readonly message: string; readonly retryAfter?: number };

export type PairProgress = (progress: { phrase: string; fingerprint: string }) => void;
export type PairRun = (code: string, name: string, progress: PairProgress, signal: AbortSignal) => Promise<Json>;

// Keep aligned with crates/aether-remote/src/auth/name.rs.
const UNSEEN: ReadonlyArray<readonly [number, number]> = [
  [0x00ad, 0x00ad], [0x034f, 0x034f], [0x0600, 0x0605], [0x061c, 0x061c], [0x06dd, 0x06dd], [0x070f, 0x070f], [0x0890, 0x0891],
  [0x08e2, 0x08e2], [0x115f, 0x1160], [0x17b4, 0x17b5], [0x180b, 0x180f], [0x200b, 0x200f], [0x202a, 0x202e], [0x2060, 0x206f],
  [0x2800, 0x2800], [0x3164, 0x3164], [0xd800, 0xdfff], [0xe000, 0xf8ff], [0xfdd0, 0xfdef], [0xfe00, 0xfe0f], [0xfeff, 0xfeff],
  [0xffa0, 0xffa0], [0xfff0, 0xfffc], [0x110bd, 0x110bd], [0x110cd, 0x110cd], [0x13430, 0x1343f], [0x1bca0, 0x1bca3],
  [0x1d173, 0x1d17a], [0xe0000, 0xe0fff], [0xf0000, 0x10ffff],
];

export function normalizeDeviceName(input: string): string {
  string(input, "Device name");
  for (const character of input) {
    const point = character.codePointAt(0) ?? 0;
    if (point < 0x20 || (point >= 0x7f && point <= 0x9f) || (point & 0xfffe) === 0xfffe || UNSEEN.some(([first, last]) => point >= first && point <= last)) {
      throw new Error("Device name cannot contain control, formatting or invisible characters. Use plain text or an emoji without a joiner or variation selector.");
    }
  }
  const normalized = input.trim().replace(/\s+/gu, " ");
  if (!normalized || [...normalized].length > 64) throw new Error("Device name must be 1–64 characters after spaces are combined.");
  return normalized;
}

async function post(fetcher: typeof fetch, path: string, body: Json, signal: AbortSignal, credentials: RequestCredentials): Promise<Json> {
  const response = await fetcher(path, { method: "POST", credentials, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body), signal });
  const result = object(await response.json(), "pairing response");
  if (response.ok) return result;
  const error = object(result.error, "pairing error");
  const retry = Number(response.headers.get("Retry-After"));
  const retryAfter = response.status === 429 && Number.isSafeInteger(retry) && retry > 0 && retry <= 3600 ? retry : undefined;
  throw new DaemonError(string(error.code, "pairing error code"), String(error.message ?? error.code), retryAfter ? { retry_after: retryAfter } : {});
}

function pause(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal.aborted) return reject(signal.reason);
    const timer = setTimeout(() => { signal.removeEventListener("abort", abort); resolve(); }, ms);
    const abort = () => { clearTimeout(timer); reject(signal.reason); };
    signal.addEventListener("abort", abort, { once: true });
  });
}

/** Enrolls a fresh device key with aetherd and saves it once the host confirms. */
export function aetherPairing(identities: IdentityStore, fetcher: typeof fetch = globalThis.fetch.bind(globalThis), pollMs = 1000): PairRun {
  return async (code, name, progress, signal) => {
    const identity = await newIdentity();
    const publicKey = new Uint8Array(await crypto.subtle.exportKey("raw", identity.keys.publicKey));
    const claim = await post(fetcher, "/api/pair/claim", { code: string(code, "pairing code"), device_name: normalizeDeviceName(name), public_key: base64url(publicKey) }, signal, "omit");
    const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", publicKey));
    const expected = [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
    const fingerprint = string(claim.fingerprint, "claim.fingerprint");
    if (fingerprint !== expected) throw new Error("Pairing fingerprint does not match this browser key");
    const deadline = Math.min(Date.parse(string(claim.expires_at, "claim.expires_at")), Date.now() + 300_000);
    if (!Number.isFinite(deadline) || deadline <= Date.now()) throw new Error("Pairing code has expired");
    progress({ phrase: string(claim.phrase, "claim.phrase"), fingerprint });
    while (Date.now() < deadline) {
      await pause(pollMs, signal);
      const status = await post(fetcher, "/api/pair/status", { pairing: string(claim.pairing, "claim.pairing") }, signal, "omit");
      const state = string(status.state, "pairing state");
      if (state === "claimed") continue;
      if (state !== "enrolled") throw new Error(`Pairing ${state}; open a new code on the host`);
      const device = object(status.device, "enrolled device");
      if (device.role !== "operator" && device.role !== "observer") throw new Error("Invalid enrolled role");
      await identities.save({ ...identity, deviceId: string(device.id, "device id"), name: string(device.name, "device name"), role: device.role });
      return device;
    }
    throw new Error("Pairing expired before host confirmation");
  };
}

/** Claims an Archon pairing code; the answer sets the device cookie under /archon. */
export function archonPairing(fetcher: typeof fetch = globalThis.fetch.bind(globalThis)): PairRun {
  return async (code, name, _progress, signal) => {
    const claimed = await post(fetcher, `${ARCHON_MOUNT}/api/pair/claim`, { code: string(code, "pairing code"), device_name: normalizeDeviceName(name) }, signal, "same-origin");
    return object(claimed.device, "claimed device");
  };
}

/** One pairing attempt at a time, as a state a screen renders. */
export class PairingFlow {
  private current: PairingState = { phase: "idle" };
  private abort: AbortController | null = null;
  private readonly listeners = new Set<() => void>();
  private readonly run: PairRun;
  private readonly onPaired: () => void;

  constructor(run: PairRun, onPaired: () => void) {
    this.run = run;
    this.onPaired = onPaired;
  }

  state = (): PairingState => this.current;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  async start(code: string, name: string): Promise<void> {
    invariant(this.current.phase !== "claiming" && this.current.phase !== "confirm", "one pairing at a time");
    const abort = (this.abort = new AbortController());
    this.set({ phase: "claiming" });
    try {
      const device = await this.run(code, name, (p) => this.set({ phase: "confirm", ...p }), abort.signal);
      if (abort.signal.aborted) return;
      this.set({ phase: "paired", device });
      this.onPaired();
    } catch (error) {
      if (abort.signal.aborted) return;
      const retry = error instanceof DaemonError ? error.details.retry_after : undefined;
      this.set({ phase: "failed", message: errorText(error), ...(typeof retry === "number" ? { retryAfter: retry } : {}) });
    }
  }

  cancel(): void {
    this.abort?.abort(new Error("Pairing cancelled"));
    this.set({ phase: "idle" });
  }

  private set(state: PairingState): void {
    this.current = state;
    for (const listener of [...this.listeners]) listener();
  }
}
