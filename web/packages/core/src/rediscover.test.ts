import type { DesktopBridge, LocalDaemon } from "@aether-ui/protocol";
import { describe, expect, it, vi } from "vitest";
import { createApp } from "./app.ts";

const storage = { get: async () => undefined, set: async () => {} };
const daemon = (kind: "aether" | "archon", reachable: boolean): LocalDaemon =>
  ({ id: `${kind}-local`, kind, label: "this computer", socket: `/run/${kind}.sock`, reachable }) as LocalDaemon;

function bridge(answers: LocalDaemon[][], managed = false): DesktopBridge {
  let call = 0;
  return {
    version: 1,
    startDaemon: vi.fn(async () => {}),
    discover: vi.fn(async () => answers[Math.min(call++, answers.length - 1)] ?? []),
    connect: vi.fn(() => new Promise<string>(() => {})),
    close: vi.fn(async () => {}),
    getPreference: async (key: string) => key === "desktop.managed_start" && managed,
  } as unknown as DesktopBridge;
}

describe("app.rediscover", () => {
  it("connects daemons that started after the app, once each", async () => {
    const desktop = bridge([[daemon("aether", false)], [daemon("aether", true), daemon("archon", true)]]);
    const app = createApp({ kind: "electron", desktop, storage }, { scheduler: (f) => f() });
    await app.start();
    expect(app.manager.all()).toHaveLength(0);
    await expect(app.rediscover()).resolves.toBe(2);
    expect(app.manager.all().map((c) => c.endpoint.daemon).sort()).toEqual(["aether", "archon"]);
    await expect(app.rediscover()).resolves.toBe(0);
    expect(app.manager.all()).toHaveLength(2);
    app.stop();
  });

  it("starts the daemons that were down at launch when managed start is on, then connects them", async () => {
    const desktop = bridge([[daemon("aether", true)], [daemon("aether", true), daemon("archon", true)]], true);
    const app = createApp({ kind: "electron", desktop, storage }, { scheduler: (f) => f() });
    await app.start();
    await vi.waitFor(() => expect(app.manager.all()).toHaveLength(2));
    expect(desktop.startDaemon).toHaveBeenCalledTimes(1);
    expect(desktop.startDaemon).toHaveBeenCalledWith("archon");
    app.stop();
  });

  it("starts nothing when managed start is off, and reports a start that failed", async () => {
    const off = bridge([[]]);
    const quiet = createApp({ kind: "electron", desktop: off, storage }, { scheduler: (f) => f() });
    await quiet.start();
    expect(off.startDaemon).not.toHaveBeenCalled();

    const failing = bridge([[]], true);
    vi.mocked(failing.startDaemon).mockRejectedValue(new Error("no aether binary"));
    const app = createApp({ kind: "electron", desktop: failing, storage }, { scheduler: (f) => f() });
    await app.start();
    await vi.waitFor(() => expect(app.store.get().reports.at(-1)?.message).toMatch(/Could not start Aether on this computer: no aether binary/));
    expect(failing.startDaemon).toHaveBeenCalledTimes(1);
  });

  it("is refused in a browser, which cannot see this computer's daemons", async () => {
    const app = createApp({ kind: "browser", storage }, { scheduler: (f) => f(), endpoints: [] });
    await expect(app.rediscover()).rejects.toMatchObject({ code: "forbidden" });
  });
});
