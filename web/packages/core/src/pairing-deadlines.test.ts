import { afterEach, describe, expect, it, vi } from "vitest";
import { aetherPairing, archonPairing, PairingFlow, type PairProgress } from "./pairing.ts";
import type { IdentityStore } from "./transports/identity.ts";

const realTimeout = globalThis.setTimeout;
const stalled = () => new Promise<never>(() => {});
const store: IdentityStore = { load: async () => undefined, save: async () => {}, remove: async () => {} };

afterEach(() => vi.useRealTimers());

describe("pairing deadlines", () => {
  it.each(["headers", "body"])("times out a stalled Archon claim %s and aborts fetch", async (stage) => {
    vi.useFakeTimers();
    let signal: AbortSignal | undefined;
    const fetcher = vi.fn((_path: RequestInfo | URL, init?: RequestInit) => {
      signal = init?.signal as AbortSignal;
      return stage === "headers" ? stalled() : Promise.resolve({ status: 200, ok: true, text: stalled } as unknown as Response);
    });
    const result = archonPairing(fetcher)("code", "phone", () => {}, new AbortController().signal);
    const assertion = expect(result).rejects.toThrow(/timed out/i);
    await vi.advanceTimersByTimeAsync(30_000);
    await assertion;
    expect(signal?.aborted).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("cancels a stalled request even when fetch ignores the signal", async () => {
    vi.useFakeTimers();
    const controller = new AbortController();
    const fetcher = vi.fn(() => stalled());
    const result = archonPairing(fetcher)("code", "phone", () => {}, controller.signal);
    const assertion = expect(result).rejects.toThrow("cancelled");
    controller.abort(new Error("cancelled"));
    await assertion;
    expect((fetcher.mock.calls[0] as unknown as [string, RequestInit])[1].signal?.aborted).toBe(true);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("accepts a successful Archon claim", async () => {
    const result = await archonPairing(async () => Response.json({ device: { id: "d" } }))("code", "phone", () => {}, new AbortController().signal);
    expect(result).toEqual({ id: "d" });
  });

  it("ignores late progress and results after cancellation and immediate retry", async () => {
    let resolveFirst: (value: { id: string }) => void = () => {};
    let progressFirst: PairProgress = () => {};
    const onPaired = vi.fn();
    const flow = new PairingFlow((_code, _name, progress) => {
      if (!progressFirstCalled) {
        progressFirstCalled = true;
        progressFirst = progress;
        return new Promise((resolve) => { resolveFirst = resolve; });
      }
      return Promise.resolve({ id: "second" });
    }, onPaired);
    let progressFirstCalled = false;
    const first = flow.start("one", "phone");
    flow.cancel();
    await flow.start("two", "phone");
    progressFirst({ phrase: "late", fingerprint: "late" });
    resolveFirst({ id: "first" });
    await first;
    expect(flow.state()).toEqual({ phase: "paired", device: { id: "second" } });
    expect(onPaired).toHaveBeenCalledTimes(1);
  });

  it("times out a stalled Aether claim", async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn(() => stalled());
    const result = aetherPairing(store, fetcher)("code", "phone", () => {}, new AbortController().signal);
    const assertion = expect(result).rejects.toThrow(/timed out/i);
    while (!fetcher.mock.calls.length) await new Promise<void>((resolve) => realTimeout(resolve, 0));
    await vi.advanceTimersByTimeAsync(30_000);
    await assertion;
    expect(vi.getTimerCount()).toBe(0);
  });

  it.each([["sleep", false], ["headers", false], ["body", false], ["headers", true], ["body", true]] as const)("bounds Aether confirmation %s (request timeout: %s)", async (stage, timeout) => {
    vi.useFakeTimers();
    const save = vi.fn(async () => {});
    let progress = false;
    const fetcher = vi.fn(async (path: RequestInfo | URL, init?: RequestInit) => {
      if (path === "/api/pair/claim") {
        const body = JSON.parse(String(init?.body)) as { public_key: string };
        const raw = Uint8Array.from(atob(body.public_key.replaceAll("-", "+").replaceAll("_", "/")), (c) => c.charCodeAt(0));
        const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", raw));
        const fingerprint = [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
        return Response.json({ pairing: "p", phrase: "phrase", fingerprint, expires_at: new Date(Date.now() + (timeout ? 60_000 : 5_000)).toISOString() });
      }
      return stage === "headers" ? stalled() : { status: 200, ok: true, text: stalled } as unknown as Response;
    });
    const result = aetherPairing({ ...store, save }, fetcher, stage === "sleep" ? 10_000 : 1)("code", "phone", () => { progress = true; }, new AbortController().signal);
    const assertion = expect(result).rejects.toThrow(timeout ? /timed out/i : /expired/i);
    while (!progress) await new Promise<void>((resolve) => realTimeout(resolve, 0));
    await vi.advanceTimersByTimeAsync(timeout ? 30_001 : 5_000);
    await assertion;
    expect(save).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });
});
