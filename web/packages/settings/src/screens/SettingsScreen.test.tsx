// @vitest-environment happy-dom
// The settings route: which page a path names, which connection the pages act on, and that the
// route is registered for `#/settings/*`.

import { matchRoute } from "@aether-ui/core";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { afterEach, describe, expect, test } from "vitest";
import { connected, type Harness } from "../kit/fixture/harness.test-util.tsx";
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
});
