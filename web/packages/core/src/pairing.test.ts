import { describe, expect, it, vi } from "vitest";
import type { Json } from "./guards.ts";
import { aetherPairing, archonPairing, normalizeDeviceName, PairingFlow } from "./pairing.ts";
import type { DeviceIdentity, IdentityStore } from "./transports/identity.ts";

function identities(): IdentityStore & { saved: DeviceIdentity[] } {
  const saved: DeviceIdentity[] = [];
  return { saved, load: async () => saved.at(-1), save: async (identity) => { saved.push(identity); }, remove: async () => {} };
}

async function fingerprintOf(body: Json): Promise<string> {
  const text = String(body.public_key).replaceAll("-", "+").replaceAll("_", "/");
  const raw = Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", raw));
  return [...digest].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** An aetherd pairing endpoint: claimed for `pending` polls, then the given final state. */
function aetherd(final: Json, pending = 1, tamper = false) {
  const calls: { path: string; body: Json; init: RequestInit }[] = [];
  const fetcher = vi.fn(async (path: string, init: RequestInit) => {
    const body = JSON.parse(String(init.body)) as Json;
    calls.push({ path, body, init });
    if (path === "/api/pair/claim") {
      const fingerprint = tamper ? "0".repeat(64) : await fingerprintOf(body);
      return Response.json({ pairing: "p1", phrase: "amber otter", fingerprint, expires_at: new Date(Date.now() + 60_000).toISOString() });
    }
    const polls = calls.filter((c) => c.path === "/api/pair/status").length;
    return Response.json(polls <= pending ? { state: "claimed" } : final);
  });
  return { fetcher: fetcher as unknown as typeof fetch, calls };
}

describe("Aether pairing", () => {
  it("shows the phrase, polls until enrolled and saves the paired identity", async () => {
    const store = identities();
    const { fetcher, calls } = aetherd({ state: "enrolled", device: { id: "dev_9", name: "phone", role: "operator" } });
    const flow = new PairingFlow(aetherPairing(store, fetcher, 1), vi.fn());
    const phases: string[] = [];
    flow.subscribe(() => phases.push(flow.state().phase));
    await flow.start("123-456", "  my   phone ");
    expect(phases).toEqual(["claiming", "confirm", "paired"]);
    expect(calls[0]?.body).toMatchObject({ code: "123-456", device_name: "my phone" });
    expect(calls.every((c) => c.init.credentials === "omit")).toBe(true);
    expect(store.saved).toEqual([expect.objectContaining({ deviceId: "dev_9", role: "operator" })]);
    expect(store.saved[0]?.keys.privateKey.extractable).toBe(false);
  });

  it("fails when the daemon's fingerprint is not of this browser's key", async () => {
    const store = identities();
    const { fetcher } = aetherd({ state: "enrolled" }, 0, true);
    const flow = new PairingFlow(aetherPairing(store, fetcher, 1), () => {});
    await flow.start("123-456", "phone");
    expect(flow.state()).toMatchObject({ phase: "failed", message: expect.stringContaining("fingerprint") });
    expect(store.saved).toEqual([]);
  });

  it("fails when the host refuses", async () => {
    const { fetcher } = aetherd({ state: "rejected" });
    const flow = new PairingFlow(aetherPairing(identities(), fetcher, 1), () => {});
    await flow.start("123-456", "phone");
    expect(flow.state()).toEqual({ phase: "failed", message: "The host rejected this device. Start a new pairing on the host to try again." });
  });
});

describe("Archon pairing", () => {
  it("claims under /archon with the cookie and passes on Retry-After", async () => {
    const fetcher = vi.fn(async () => Response.json({ error: { code: "rate_limited", message: "Slow down" } }, { status: 429, headers: { "Retry-After": "30" } }));
    const flow = new PairingFlow(archonPairing(fetcher as unknown as typeof fetch), () => {});
    await flow.start("ABCD", "phone");
    expect(fetcher).toHaveBeenCalledWith("/archon/api/pair/claim", expect.objectContaining({ method: "POST", credentials: "same-origin" }));
    expect(flow.state()).toEqual({ phase: "failed", message: "Too many codes were tried just now. Wait 30 seconds and try again.", retryAfter: 30 });
  });
});

describe("pairing failures read as plain sentences", () => {
  async function failure(answer: () => Response | Promise<Response>) {
    const fetcher = vi.fn(async () => answer());
    const flow = new PairingFlow(archonPairing(fetcher as unknown as typeof fetch), () => {});
    await flow.start("ABCD-1234", "phone");
    expect(fetcher).toHaveBeenCalledTimes(1);
    return flow.state();
  }
  const refusal = (code: string, message: string, status: number) => Response.json({ error: { code, message } }, { status });

  it("an empty body is not a pairing service, whatever the status", async () => {
    expect(await failure(() => new Response(null, { status: 404 })))
      .toEqual({ phase: "failed", message: "The host did not answer as a pairing service (HTTP 404). Check the address." });
    expect(await failure(() => new Response("", { status: 200 })))
      .toMatchObject({ message: "The host did not answer as a pairing service (HTTP 200). Check the address." });
  });

  it("an HTML page or a JSON body without an error is not a pairing service", async () => {
    const html = await failure(() => new Response("<!doctype html><title>Welcome</title>", { status: 200, headers: { "Content-Type": "text/html" } }));
    expect(html).toMatchObject({ phase: "failed", message: expect.stringContaining("did not answer as a pairing service (HTTP 200)") });
    expect(html).not.toMatchObject({ message: expect.stringMatching(/JSON|Unexpected/) });
    expect(await failure(() => Response.json({ detail: "oops" }, { status: 500 }))).toMatchObject({ message: expect.stringContaining("(HTTP 500)") });
  });

  it("a wrong code says to check it", async () => {
    expect(await failure(() => refusal("invalid_code", "that code is not open for pairing", 403)))
      .toEqual({ phase: "failed", message: "That code is not open for pairing. Check it against the host, or start a new pairing there." });
  });

  it("an expired pairing says to start a new one", async () => {
    expect(await failure(() => refusal("expired", "the pairing expired; start a new one on the host", 410)))
      .toEqual({ phase: "failed", message: "That pairing expired. Start a new one on the host and type its new code." });
  });

  it("429 keeps Retry-After, and a refusal without a usable one waits a moment", async () => {
    const limited = (retry: string) => Response.json({ error: { code: "rate_limited", message: "too many pairing attempts; wait and try again" } }, { status: 429, headers: { "Retry-After": retry } });
    expect(await failure(() => limited("1"))).toEqual({ phase: "failed", message: "Too many codes were tried just now. Wait 1 second and try again.", retryAfter: 1 });
    expect(await failure(() => limited("soon"))).toEqual({ phase: "failed", message: "Too many codes were tried just now. Wait a moment and try again." });
  });

  it("an unknown code keeps the daemon's message, and a prototype key is not a known code", async () => {
    expect(await failure(() => refusal("bad_request", "device_name must be 1 to 64 characters", 400))).toMatchObject({ message: "device_name must be 1 to 64 characters" });
    expect(await failure(() => refusal("constructor", "", 400))).toMatchObject({ message: "The host refused to pair (constructor)." });
  });

  it("a host that cannot be reached says so", async () => {
    expect(await failure(() => Promise.reject(new TypeError("Failed to fetch"))))
      .toEqual({ phase: "failed", message: "Could not reach the host. Check the address and the network, then try again." });
  });
});

describe("normalizeDeviceName", () => {
  it("combines spaces and refuses invisible characters", () => {
    expect(normalizeDeviceName(" a    b ")).toBe("a b");
    expect(() => normalizeDeviceName("a​b")).toThrow(/invisible/);
    expect(() => normalizeDeviceName("   ")).toThrow(/1–64/);
  });
});
