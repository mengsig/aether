// @vitest-environment happy-dom
// The settings route: which page a path names, which connection the pages act on, and that the
// route is registered for `#/settings/*`.

import { matchRoute } from "@aether-ui/core";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { afterEach, describe, expect, test } from "vitest";
import { ScriptedDaemon } from "../kit/fixture/daemon.ts";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import { click, connected, type Harness } from "../kit/fixture/harness.test-util.tsx";
import routes from "./routes.ts";
import SettingsScreen, { pageOf, pickConnection } from "./SettingsScreen.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

const info = (id: string, state: ConnectionInfo["state"]): ConnectionInfo => ({ id, daemon: "aether", state, features: [], label: id });

describe("settings route", () => {
  test("#/settings and its sub-pages match the one route, with the page as the rest", () => {
    expect(matchRoute(routes, "#/settings")?.params["*"]).toBe("");
    expect(matchRoute(routes, "#/settings/devices")?.params["*"]).toBe("devices");
    expect(routes[0]).toMatchObject({ owner: "B3", nav: { label: "Settings", section: "settings" } });
  });

  test("an unknown page falls back to the connections page", () => {
    expect(pageOf(undefined)).toBe("connections");
    expect(pageOf("nonsense")).toBe("connections");
    expect(pageOf("remote/extra")).toBe("remote");
  });

  test("the first ready connection is used unless the person chose another that still exists", () => {
    const list = [info("a", "backoff"), info("b", "ready"), info("c", "ready")];
    expect(pickConnection(list, null)?.id).toBe("b");
    expect(pickConnection(list, "c")?.id).toBe("c");
    expect(pickConnection(list, "gone")?.id).toBe("b");
    expect(pickConnection([info("a", "backoff")], null)?.id).toBe("a");
    expect(pickConnection([], null)).toBeUndefined();
  });
});

describe("SettingsScreen", () => {
  test("shows the connections page for #/settings and the devices page for #/settings/devices", async () => {
    const h = current = await connected({ role: "operator", script: { list_devices: () => ({ tag: "devices", data: [{ id: "dev_a", name: "Hall tablet", role: "observer", created_at: "2026-10-01T00:00:00Z" }] }) } });
    let root = await h.mount(<SettingsScreen params={{ "*": "" }} />);
    expect(root.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("Connections");
    expect(root.textContent).toContain("studio");
    h.unmount();

    const again = current = await connected({ role: "operator", script: { list_devices: () => ({ tag: "devices", data: [{ id: "dev_a", name: "Hall tablet", role: "observer", created_at: "2026-10-01T00:00:00Z" }] }) } });
    root = await again.mount(<SettingsScreen params={{ "*": "devices" }} />);
    expect(root.querySelector('[role="tab"][aria-selected="true"]')?.textContent).toBe("Devices");
    expect(root.textContent).toContain("Hall tablet");
  });

  test("UX-043: a running repair keeps Settings on its Aether while it restarts, and its failure shows there once", async () => {
    let fail: (error: Error) => void = () => undefined;
    const bridge = Object.assign(fakeDesktop().bridge, { setup: () => new Promise((_, reject) => { fail = reject; }) });
    const missing = { mcp_servers: () => ({ tag: "mcp_servers", data: [] }), providers: () => ({ tag: "providers", data: [] }), auth_status: () => ({ tag: "auth", data: [] }) };
    const laptop = new ScriptedDaemon({ instance: "laptop", role: "operator", script: missing });
    const h = current = await connected({ role: "local", desktop: bridge, script: missing, others: [{ daemon: laptop, label: "laptop" }] });
    expect(h.app.store.get().connections["aether:laptop"]?.state).toBe("ready");
    const root = await h.mount(<SettingsScreen params={{ "*": "providers" }} />);
    const shown = () => root.querySelector<HTMLSelectElement>(".st-picker select")?.value;
    expect(shown()).toBe(h.conn);
    await click(h, root, "Connect Archon tools");

    h.daemon.drop();
    await h.settle();
    expect(h.app.store.get().connections[h.conn]?.state).not.toBe("ready");
    expect(shown()).toBe(h.conn);
    expect(root.querySelector('.su-step[data-state="running"]')?.textContent).toContain("Connect Archon's tools to Aether");

    fail(new Error("Archon could not be reached. Is it installed?"));
    await h.settle();
    expect(root.querySelector('.su-step[data-state="failed"]')?.textContent).toContain("Archon could not be reached. Is it installed?");
    expect(h.app.store.get().reports.filter((r) => r.message.startsWith("Connecting Archon's tools stopped"))).toHaveLength(1);
  });
});
