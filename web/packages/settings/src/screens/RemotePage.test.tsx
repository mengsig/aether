// @vitest-environment happy-dom
// Remote: readable and changeable only by the host, through the desktop's confirmation.

import { afterEach, describe, expect, test } from "vitest";
import { connected, click, buttonNamed, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import { RemotePage } from "./RemotePage.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });


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

  test("the host is told the status cannot be read yet, and no get_remote is sent", async () => {
    const h = current = await connected({ role: "local", desktop: fakeDesktop().bridge });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    expect(h.daemon.commands("get_remote")).toHaveLength(0);
    expect(root.querySelector("[role=note]")?.textContent).toContain("The desktop does not pass this host-only read to the daemon yet");
    expect(buttonNamed(root, "Turn on").disabled).toBe(false);
    expect(buttonNamed(root, "Turn off").disabled).toBe(false);
  });

  test("turning it off asks first, then goes through the desktop", async () => {
    const desktop = fakeDesktop();
    const h = current = await connected({ role: "local", desktop: desktop.bridge });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    await click(h, root, "Turn off");
    expect(desktop.actions).toHaveLength(0);
    await click(h, document.body, "Turn off remote access");
    expect(desktop.actions).toEqual([{ kind: "remote.disable", conn: "bridge-1" }]);
    expect(root.textContent).toContain("Remote access turned off");
  });

  test("turning it on sends remote.enable", async () => {
    const desktop = fakeDesktop();
    const h = current = await connected({ role: "local", desktop: desktop.bridge });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    await click(h, root, "Turn on");
    await click(h, document.body, "Turn on remote access");
    expect(desktop.actions).toEqual([{ kind: "remote.enable", conn: "bridge-1" }]);
  });
});
