// @vitest-environment happy-dom
import type { aether } from "@aether-ui/protocol";
import { act } from "react";
import { afterEach, describe, expect, test } from "vitest";
import { connected, click, buttonNamed, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import { RemotePage } from "./RemotePage.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });
const ADDRESS = "127.0.0.1:9843";
const remote = (enabled: boolean): aether.RemoteStatus => ({
  address: ADDRESS, revision: 1, settings: { enabled }, listening: enabled ? ADDRESS : null, devices: 2, connections: enabled ? 1 : 0,
});

async function local(status: aether.RemoteStatus) {
  const desktop = fakeDesktop();
  const h = current = await connected({ role: "local", desktop: desktop.bridge, script: { get_remote: () => ({ tag: "remote", data: status }) } });
  const root = await h.mount(<RemotePage conn={h.conn} />);
  expect(h.daemon.commands("get_remote")).toHaveLength(1);
  expect(root.querySelector(".ui-badge")?.textContent).toBe(status.settings.enabled ? "On" : "Off");
  return { h, root, desktop };
}

async function confirm(h: Harness, root: HTMLElement, enabled: boolean) {
  const label = enabled ? "Turn on remote access" : "Turn off remote access";
  await click(h, root, label);
  const dialog = document.querySelector("dialog[open]");
  expect(dialog).not.toBeNull();
  expect(dialog?.textContent).toContain(`${label}?`);
  if (dialog === null) throw new Error("the confirmation dialog is open");
  await click(h, dialog, label);
  await h.settle();
}

describe("RemotePage", () => {
  test.each(["operator", "observer"] as const)("a remote %s cannot read or change host settings", async (role) => {
    const h = current = await connected({ role });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    expect(h.daemon.commands("get_remote")).toHaveLength(0);
    expect(root.textContent).toContain("Only the daemon's own host can do this");
    expect(root.textContent).toContain("Host only");
    expect(root.querySelectorAll("button")).toHaveLength(0);
  });

  test("off shows only Turn on and the saved state survives a remount", async () => {
    const { root } = await local(remote(false));
    expect(buttonNamed(root, "Turn on remote access").disabled).toBe(false);
    expect(root.textContent).not.toContain("Turn off remote access");
    expect(root.textContent).toContain("Not listening");
    current?.unmount();
    current = null;
    const reloaded = await local(remote(false));
    expect(reloaded.root.querySelector(".ui-badge")?.textContent).toBe("Off");
    expect(reloaded.h.daemon.commands("get_remote")).toHaveLength(1);
  });

  test("on shows only Turn off with the listener, address and counts", async () => {
    const { root } = await local(remote(true));
    expect(buttonNamed(root, "Turn off remote access").disabled).toBe(false);
    expect(root.textContent).not.toContain("Turn on remote access");
    expect(root.textContent).toContain(`Address: ${ADDRESS}`);
    expect(root.textContent).toContain(`Listening on ${ADDRESS}`);
    expect(root.textContent).toContain("Paired devices: 2");
    expect(root.textContent).toContain("Connections: 1");
  });

  test("the confirmation can be cancelled without changing the host", async () => {
    const { h, root, desktop } = await local(remote(false));
    await click(h, root, "Turn on remote access");
    expect(desktop.actions).toHaveLength(0);
    expect(document.querySelector("dialog[open]")?.textContent).toContain("Paired devices can reach this daemon from its public origins.");
    await click(h, document.body, "Keep it");
    expect(desktop.actions).toHaveLength(0);
    expect(h.daemon.commands("get_remote")).toHaveLength(1);
  });

  test("turning on confirms, sends remote.enable, then verifies a fresh reply", async () => {
    const { h, root, desktop } = await local(remote(false));
    h.daemon.script = { get_remote: () => "hold" };
    await confirm(h, root, true);
    expect(desktop.actions).toEqual([{ kind: "remote.enable", conn: "bridge-1" }]);
    expect(h.daemon.commands("get_remote")).toHaveLength(2);
    expect(root.querySelector("[role=status]")?.textContent).toBe("Checking remote access…");
    expect(buttonNamed(root, "Turn on remote access").disabled).toBe(true);
    h.daemon.script = { get_remote: () => ({ tag: "remote", data: remote(true) }) };
    await act(async () => h.daemon.release());
    await h.settle();
    expect(root.querySelector("[role=status]")?.textContent).toBe(`Verified: remote access is on, listening on ${ADDRESS}`);
    expect(root.querySelector(".ui-badge")?.textContent).toBe("On");
    expect(buttonNamed(root, "Turn off remote access").disabled).toBe(false);
    expect(root.textContent).not.toContain("Turn on remote access");
  });

  test("turning off sends remote.disable and verifies off", async () => {
    const { h, root, desktop } = await local(remote(true));
    h.daemon.script = { get_remote: () => ({ tag: "remote", data: remote(false) }) };
    await confirm(h, root, false);
    expect(desktop.actions).toEqual([{ kind: "remote.disable", conn: "bridge-1" }]);
    expect(root.querySelector("[role=status]")?.textContent).toBe("Verified: remote access is off");
    expect(buttonNamed(root, "Turn on remote access").disabled).toBe(false);
  });

  test("a mismatch alerts that the host did not apply it and allows retry", async () => {
    const { h, root, desktop } = await local(remote(false));
    await confirm(h, root, true);
    expect(root.querySelector("[role=alert]")?.textContent).toContain("The host did not apply the change");
    expect(root.textContent).toContain("Remote access is still off");
    expect(root.textContent).not.toContain("Verified:");
    h.daemon.script = { get_remote: () => ({ tag: "remote", data: remote(true) }) };
    await click(h, root, "Retry verification");
    expect(root.querySelector("[role=status]")?.textContent).toContain("Verified: remote access is on");
    expect(desktop.actions).toHaveLength(1);
  });

  test("a read error shows Retry and no toggle until the read succeeds", async () => {
    const h = current = await connected({ role: "local", desktop: fakeDesktop().bridge,
      script: { get_remote: () => ({ code: "unavailable", message: "Remote status unavailable" }) } });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    expect(root.querySelector("[role=alert]")?.textContent).toContain("Remote status unavailable");
    expect(root.textContent).not.toContain("Turn on remote access");
    h.daemon.script = { get_remote: () => ({ tag: "remote", data: remote(false) }) };
    await click(h, root, "Retry");
    expect(buttonNamed(root, "Turn on remote access").disabled).toBe(false);
    expect(root.querySelector("[role=alert]")).toBeNull();
  });

  test("a failed verification read cannot verify the cached state", async () => {
    const { h, root } = await local(remote(false));
    h.daemon.script = { get_remote: () => ({ code: "unavailable", message: "Refresh failed" }) };
    await confirm(h, root, true);
    expect(root.textContent).toContain("Could not verify remote access");
    expect(root.textContent).toContain("Refresh failed");
    expect(root.textContent).not.toContain("Verified:");
    expect(buttonNamed(root, "Turn on remote access").disabled).toBe(true);
    h.daemon.script = { get_remote: () => ({ tag: "remote", data: remote(true) }) };
    await click(h, root, "Retry verification");
    expect(root.querySelector("[role=status]")?.textContent).toContain("Verified: remote access is on");
  });

  test("loading hides controls and listener errors use a danger badge and alert", async () => {
    const h = current = await connected({ role: "local", desktop: fakeDesktop().bridge, script: { get_remote: () => "hold" } });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    expect(root.querySelector('[aria-label="Loading remote access"]')?.getAttribute("aria-busy")).toBe("true");
    expect(root.querySelectorAll("button")).toHaveLength(0);
    h.daemon.script = { get_remote: () => ({ tag: "remote", data: { ...remote(true), listening: null, error: "Address in use" } }) };
    await act(async () => h.daemon.release());
    await h.settle();
    expect(root.querySelector(".ui-badge")?.getAttribute("data-tone")).toBe("danger");
    expect(root.querySelector("[role=alert]")?.textContent).toBe("Address in use");
    expect(root.textContent).toContain("Not listening");
  });

  test("an invalid status cannot produce a toggle", async () => {
    const h = current = await connected({ role: "local", desktop: fakeDesktop().bridge,
      script: { get_remote: () => ({ tag: "remote", data: { ...remote(false), settings: null } }) } });
    const root = await h.mount(<RemotePage conn={h.conn} />);
    expect(root.querySelector("[role=alert]")?.textContent).toContain("invalid remote status");
    expect(root.textContent).not.toContain("Turn on remote access");
    expect(buttonNamed(root, "Retry").disabled).toBe(false);
  });
});
