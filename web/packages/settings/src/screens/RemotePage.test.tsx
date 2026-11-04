// @vitest-environment happy-dom
// Remote: readable and changeable only by the host, through the desktop's confirmation.

import { afterEach, describe, expect, test } from "vitest";
import { connected, click, buttonNamed, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import { RemotePage } from "./RemotePage.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

const status = (enabled: boolean) => ({
  address: "127.0.0.1:7777", listening: enabled ? "0.0.0.0:7777" : null, connections: 2, devices: 3, revision: 4,
  settings: { enabled, public_origins: ["https://aether.example.net"], mounts: [{ kind: "archon", prefix: "/archon", target: "127.0.0.1:9000" }] },
});

describe("RemotePage", () => {
  test("a remote operator does not read get_remote and is told why", async () => {
    const h = current = await connected({ role: "operator" });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    expect(h.daemon.commands("get_remote")).toHaveLength(0);
    expect(root.textContent).toContain("Only the daemon's own host can do this");
    expect(root.textContent).toContain("Host only");
  });

  test("an observer does not read it either, and has no control to press", async () => {
    const h = current = await connected({ role: "observer" });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    expect(h.daemon.commands("get_remote")).toHaveLength(0);
    expect(root.querySelectorAll("button")).toHaveLength(0);
    expect(root.textContent).toContain("Only the daemon's own host can do this");
  });

  test("the host sees the state, origins and mounts from the stored reply", async () => {
    const h = current = await connected({ role: "local", desktop: fakeDesktop().bridge, script: { get_remote: () => ({ tag: "remote", data: status(true) }) } });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    expect(root.textContent).toContain("Enabled");
    expect(root.textContent).toContain("https://aether.example.net");
    expect(root.textContent).toContain("/archon → 127.0.0.1:9000 (archon)");
    expect(h.app.store.get().reads[h.conn]?.["settings.remote"]?.value).toEqual(status(true));
  });

  test("turning it off asks first, goes through the desktop, then rereads", async () => {
    const desktop = fakeDesktop();
    let enabled = true;
    const h = current = await connected({ role: "local", desktop: desktop.bridge, script: { get_remote: () => ({ tag: "remote", data: status(enabled) }) } });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    await click(h, root, "Turn off");
    expect(desktop.actions).toHaveLength(0);
    enabled = false;
    await click(h, document.body, "Turn off remote access");
    expect(desktop.actions).toEqual([{ kind: "remote.disable", conn: "bridge-1" }]);
    expect(h.daemon.commands("get_remote")).toHaveLength(2);
    expect(root.textContent).toContain("Disabled");
    expect(buttonNamed(root, "Turn on").disabled).toBe(false);
  });

  test("turning it on sends remote.enable", async () => {
    const desktop = fakeDesktop();
    const h = current = await connected({ role: "local", desktop: desktop.bridge, script: { get_remote: () => ({ tag: "remote", data: status(false) }) } });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    await click(h, root, "Turn on");
    await click(h, document.body, "Turn on remote access");
    expect(desktop.actions).toEqual([{ kind: "remote.enable", conn: "bridge-1" }]);
  });
});
