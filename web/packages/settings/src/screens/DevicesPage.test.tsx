// @vitest-environment happy-dom
// Devices: listed from the stored reply, revoked and paired only by the host through the desktop's
// confirmation, with the list reread into core's store after a revoke.

import { afterEach, describe, expect, test } from "vitest";
import { connected, click, buttonNamed, type, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import { DevicesPage } from "./DevicesPage.tsx";

let current: Harness | null = null;
afterEach(() => { current?.unmount(); current = null; });

const phone = { id: "dev_phone", name: "Pixel phone", role: "operator", created_at: "2026-10-01T08:00:00Z", last_seen_at: "2026-11-03T10:00:00Z", current: true, push: true };
const tablet = { id: "dev_tablet", name: "Kitchen tablet", role: "observer", created_at: "2026-10-05T08:00:00Z" };

describe("DevicesPage", () => {
  test("lists the devices from the stored reply, with the current and push marks", async () => {
    const h = current = await connected({ role: "operator", script: { list_devices: () => ({ tag: "devices", data: [phone, tablet] }) } });
    const root = await h.mount(<DevicesPage conn={h.conn} />);
    expect(h.daemon.commands("list_devices")).toEqual([{ cmd: "list_devices", req: expect.any(Number) }]);
    expect(root.textContent).toContain("Pixel phone");
    expect(root.textContent).toContain("This device");
    expect(root.textContent).toContain("Push on");
    expect(root.textContent).toContain("Kitchen tablet");
    expect(h.app.store.get().reads[h.conn]?.["settings.devices"]?.value).toEqual([phone, tablet]);
  });

  test("a remote operator may list but not revoke or pair: host-only controls are marked and refused", async () => {
    const h = current = await connected({ role: "operator", script: { list_devices: () => ({ tag: "devices", data: [tablet] }) } });
    const root = await h.mount(<DevicesPage conn={h.conn} />);
    expect(buttonNamed(root, "Revoke").disabled).toBe(true);
    expect(buttonNamed(root, "Pair a new device").disabled).toBe(true);
    expect(root.textContent).toContain("Host only");
    expect(root.textContent).toContain("Only the daemon's own host can do this");
    expect(root.textContent).not.toContain("Show revoked devices");
  });

  test("an observer reads nothing, and every control is disabled with its reason", async () => {
    const h = current = await connected({ role: "observer" });
    const root = await h.mount(<DevicesPage conn={h.conn} />);
    expect(h.daemon.commands("list_devices")).toHaveLength(0);
    expect(root.textContent).toContain("Observer devices cannot change daemon state");
    expect(buttonNamed(root, "Pair a new device").disabled).toBe(true);
    expect(root.querySelector(".st-reason")?.textContent).toBe("Only the daemon's own host can do this");
  });

  test("the host is not offered revoked devices, since that listing is host-only and not passed through", async () => {
    const h = current = await connected({ role: "local", desktop: fakeDesktop().bridge, script: { list_devices: () => ({ tag: "devices", data: [tablet] }) } });
    const root = await h.mount(<DevicesPage conn={h.conn} />);
    expect(root.textContent).toContain("Kitchen tablet");
    expect(root.textContent).not.toContain("Show revoked devices");
    expect(h.daemon.commands("list_devices").every((c) => c.include_revoked !== true)).toBe(true);
  });

  test("revoking asks first, then goes through the desktop and rereads the list into the store", async () => {
    const desktop = fakeDesktop();
    let revoked = false;
    const h = current = await connected({ role: "local", desktop: desktop.bridge, script: { list_devices: () => ({ tag: "devices", data: revoked ? [] : [tablet] }) } });
    const root = await h.mount(<DevicesPage conn={h.conn} />);
    await click(h, root, "Revoke");
    expect(desktop.actions).toHaveLength(0);
    revoked = true;
    await click(h, document.body, "Revoke device");
    expect(desktop.actions).toEqual([{ kind: "device.revoke", device: "dev_tablet", conn: "bridge-1" }]);
    expect(h.daemon.commands("list_devices")).toHaveLength(2);
    expect(h.app.store.get().reads[h.conn]?.["settings.devices"]?.value).toEqual([]);
    expect(root.textContent).toContain("No devices paired");
    expect(root.querySelector(".st-note")?.textContent).toBe("Kitchen tablet was revoked");
  });

  test("pairing goes through the desktop and renders nothing the pairing returns", async () => {
    const desktop = fakeDesktop({ code: "482913", phrase: "amber-otter-lime", token: "sk-secret-credential" });
    const h = current = await connected({ role: "local", desktop: desktop.bridge, script: { list_devices: () => ({ tag: "devices", data: [] }) } });
    const root = await h.mount(<DevicesPage conn={h.conn} />);
    expect(buttonNamed(root, "Pair a new device").disabled).toBe(true);
    await type(h, root, "Device name", "  Hall   display ");
    await type(h, root, "Role", "operator");
    await click(h, root, "Pair a new device");
    expect(desktop.actions).toHaveLength(0);
    await click(h, document.body, "Start pairing");
    expect(desktop.actions).toEqual([{ kind: "device.pair", name: "Hall display", role: "operator", conn: "bridge-1" }]);
    expect(document.body.textContent).toContain("Pairing started; finish it in the desktop dialog");
    for (const secret of ["482913", "amber-otter-lime", "sk-secret-credential"]) expect(document.body.textContent).not.toContain(secret);
  });

  test("a failed read shows the error with a retry that rereads", async () => {
    let fail = true;
    const h = current = await connected({ role: "operator", script: { list_devices: () => (fail ? { code: "internal", message: "store is locked" } : { tag: "devices", data: [tablet] }) } });
    const root = await h.mount(<DevicesPage conn={h.conn} />);
    expect(root.textContent).toContain("store is locked");
    fail = false;
    await click(h, root, "Retry");
    expect(root.textContent).toContain("Kitchen tablet");
  });
});
