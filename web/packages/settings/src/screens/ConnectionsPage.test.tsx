// @vitest-environment happy-dom
// Connections: every connection in core's store with its state and standing, a link to pair again,
// a reconnect that is a view action (no daemon command), and a confirmed forget of this browser's identity.

import { afterEach, describe, expect, test, vi } from "vitest";
import { connected, click, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { ConnectionsPage } from "./ConnectionsPage.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

describe("ConnectionsPage", () => {
  test("shows the connection's label, daemon, state, protocol, standing and feature count", async () => {
    const h = current = await connected({ role: "observer", features: ["deltas", "agents_paging"] });
    const root = await h.mount(<ConnectionsPage />);
    const row = root.querySelector("li");
    expect(row?.getAttribute("aria-label")).toBe("studio");
    expect(row?.textContent).toContain("Aether");
    expect(row?.textContent).toContain("ready");
    expect(row?.textContent).toContain("v26");
    expect(row?.textContent).toContain("Observer");
    expect(row?.textContent).toContain("2 features");
    expect(row?.querySelector("a")?.getAttribute("href")).toBe("#/pair/aether");
  });

  test("a local connection says it is the host's own", async () => {
    const h = current = await connected({ role: "local" });
    const root = await h.mount(<ConnectionsPage />);
    expect(root.textContent).toContain("Local (this host)");
  });

  test("Reconnect now retries the connection and sends no daemon command", async () => {
    const h = current = await connected({ role: "operator" });
    const connection = h.app.manager.get(h.conn);
    if (connection === undefined) throw new Error("the harness connection is known to the manager");
    const retry = vi.spyOn(connection, "retryNow");
    const root = await h.mount(<ConnectionsPage />);
    const before = h.daemon.sent.length;
    await click(h, root, "Reconnect now");
    expect(retry).toHaveBeenCalledTimes(1);
    expect(h.daemon.sent).toHaveLength(before);
  });

  test("Forget this device asks first, closes the signed-in link, removes the identity, reconnects and opens pairing", async () => {
    const identities = { load: vi.fn(async () => undefined), save: vi.fn(async () => {}), remove: vi.fn(async () => {}) };
    const h = current = await connected({ role: "operator", identities });
    const connection = h.app.manager.get(h.conn);
    if (connection === undefined) throw new Error("the harness connection is known to the manager");
    const states: string[] = [];
    identities.remove.mockImplementation(async () => { states.push(connection.current.state); });
    const stop = vi.spyOn(connection, "stop");
    const start = vi.spyOn(connection, "start");
    const root = await h.mount(<ConnectionsPage />);
    await click(h, root, "Forget this device");
    await click(h, document.querySelector("dialog[open]") ?? document.body, "Keep it");
    expect(identities.remove).not.toHaveBeenCalled();
    await click(h, root, "Forget this device");
    await click(h, document.querySelector("dialog[open]") ?? document.body, "Forget this device");
    expect(identities.remove).toHaveBeenCalledTimes(1);
    expect(states).toEqual(["closed"]);
    expect(stop.mock.invocationCallOrder[0]).toBeLessThan(identities.remove.mock.invocationCallOrder[0] ?? 0);
    expect(start.mock.invocationCallOrder[0]).toBeGreaterThan(identities.remove.mock.invocationCallOrder[0] ?? Infinity);
    expect(globalThis.location.hash).toBe("#/pair/aether");
  });

  test("the host's own connection has no browser identity to forget", async () => {
    const h = current = await connected({ role: "local" });
    const root = await h.mount(<ConnectionsPage />);
    expect(root.textContent).not.toContain("Forget this device");
  });
});
