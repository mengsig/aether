// @vitest-environment happy-dom
// UX-001: the overview offers one clear way to set up this computer, and setup shows each step with
// a plain reason and a fix when one stops.

import { AppProvider, createApp, type App } from "@aether-ui/core";
import type { DesktopBridge, SetupStep } from "@aether-ui/protocol";
import { ThemeProvider } from "@aether-ui/ui";
import { act, useEffect, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, test, vi } from "vitest";
import { click, connected, type Harness } from "../kit/fixture/harness.test-util.tsx";
import { fakeDesktop } from "../kit/fixture/desktop.test-util.ts";
import SetupScreen from "./SetupScreen.tsx";
import { Welcome } from "./Welcome.tsx";

let current: Harness | null = null;
let bare: { root: Root; app: App } | null = null;
afterEach(() => {
  current?.unmount();
  current = null;
  if (bare) act(() => bare?.root.unmount());
  bare?.app.stop();
  bare = null;
  document.body.innerHTML = "";
});

function setupDesktop(answer: (step: SetupStep) => unknown) {
  const desktop = fakeDesktop();
  const calls: SetupStep[] = [];
  const openExternal = vi.fn(async () => {});
  const bridge = Object.assign(desktop.bridge, {
    openExternal, discover: async () => [],
    setup: async (step: SetupStep) => { calls.push(step); return answer(step); },
  }) as DesktopBridge;
  return { bridge, calls, openExternal };
}

/** Mounts with no daemon connected at all, as on a computer's first launch. */
async function firstLaunch(node: React.ReactNode, desktop: DesktopBridge | undefined, managed = false): Promise<HTMLElement> {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  const storage = { get: async () => undefined, set: async () => {} };
  const host = desktop ? { kind: "electron" as const, desktop: Object.assign(desktop, { getPreference: async () => managed }), storage } : { kind: "browser" as const, storage };
  const app = createApp(host, { scheduler: (flush) => flush(), endpoints: [] });
  await app.start();
  const container = document.createElement("main");
  document.body.append(container);
  const root = createRoot(container);
  bare = { root, app };
  await act(async () => root.render(<AppProvider app={app}><ThemeProvider initialPreference="light">{node}</ThemeProvider></AppProvider>));
  return container;
}

describe("Welcome", () => {
  test("a first launch on the desktop offers setup first and pairing second", async () => {
    const root = await firstLaunch(<Welcome />, setupDesktop(() => null).bridge);
    const choices = [...root.querySelectorAll(".su-choice")];
    expect(choices.map((c) => c.getAttribute("href"))).toEqual(["#/setup", "#/pair/aether"]);
    expect(choices[0]?.textContent).toContain("Set up on this computer");
    expect(root.textContent).not.toContain("Pick a screen");
  });

  test("a browser offers pairing only, and a managed re-launch waits instead of offering setup", async () => {
    const browser = await firstLaunch(<Welcome />, undefined);
    expect([...browser.querySelectorAll(".su-choice")].map((c) => c.getAttribute("href"))).toEqual(["#/pair/aether"]);
    act(() => bare?.root.unmount());
    bare?.app.stop();
    document.body.innerHTML = "";
    const relaunch = await firstLaunch(<Welcome />, setupDesktop(() => null).bridge, true);
    expect(relaunch.querySelector('[role="status"]')?.textContent).toBe("Starting Aether and Archon on this computer…");
    expect(relaunch.querySelector(".su-choice")).toBeNull();
  });

  test("this computer's Aether running without Archon's tools offers to finish setup; a paired one does not", async () => {
    const desktop = setupDesktop(() => { throw new Error("Archon could not be reached. Is it installed?"); });
    const none = { mcp_servers: () => ({ tag: "mcp_servers", data: [] }) };
    const h = current = await connected({ role: "local", desktop: desktop.bridge, script: none });
    const root = await h.mount(<Welcome />);
    expect(root.textContent).toContain("Archon's tools are not connected to Aether yet. Connect them to finish setting up this computer.");
    await click(h, root, "Connect Archon tools");
    expect(desktop.calls).toEqual(["connect"]);
    h.unmount();
    const paired = current = await connected({ role: "operator", desktop: setupDesktop(() => null).bridge, script: none });
    const pairedRoot = await paired.mount(<Welcome />);
    expect(pairedRoot.querySelector('a[href="#/activity/start"]')).not.toBeNull();
    expect(pairedRoot.textContent).not.toContain("Connect Archon tools");
  });

  test("a home check that fails or reads oddly says so with Check again and the setup path, and recovers", async () => {
    let answer: () => { tag: string; data: unknown } | { code: string; message: string } = () => ({ code: "internal", message: "the tool servers are busy" });
    const h = current = await connected({ role: "local", desktop: setupDesktop(() => null).bridge, script: { mcp_servers: () => answer() } });
    const root = await h.mount(<Welcome />);
    const group = () => root.querySelector('[role="group"][aria-label="Check Archon\'s tools"]');
    expect(group()?.querySelector('[role="alert"]')?.textContent).toContain("The app could not check Archon's tools: the tool servers are busy");
    expect(group()?.querySelector('a[href="#/setup"]')?.textContent).toBe("Set up on this computer");

    answer = () => ({ tag: "mcp_servers", data: { servers: [] } });
    await click(h, root, "Check again");
    expect(group()?.textContent).toContain("The app could not read the tool servers Aether reported.");

    answer = () => ({ tag: "mcp_servers", data: [] });
    await click(h, root, "Check again");
    expect(group()).toBeNull();
    expect(root.textContent).toContain("Archon's tools are not connected to Aether yet. Connect them to finish setting up this computer.");
    expect(h.daemon.commands("mcp_servers").filter((c) => c.refresh === true)).toHaveLength(2);
  });

  test("a failed recheck is shown over a cached good answer, and a malformed server entry reads as unreadable", async () => {
    const ready = { name: "archon", state: "ready", transport: "stdio", tools: [{ name: "queue" }] };
    let answer: () => { tag: string; data: unknown } | { code: string; message: string } = () => ({ tag: "mcp_servers", data: [ready] });
    const h = current = await connected({ role: "local", desktop: setupDesktop(() => null).bridge, script: { mcp_servers: () => answer() } });
    const root = await h.mount(<Welcome />);
    const group = () => root.querySelector('[role="group"][aria-label="Check Archon\'s tools"]');
    expect(group()).toBeNull();
    expect(root.textContent).not.toContain("Connect them to finish");

    answer = () => ({ code: "internal", message: "Aether restarted" });
    await act(async () => { await h.app.reread(h.conn as never, "settings.mcp", { cmd: "mcp_servers", refresh: true }, ["mcp_servers"]); });
    await h.settle();
    expect(group()?.querySelector('[role="alert"]')?.textContent).toContain("The app could not check Archon's tools: Aether restarted");

    for (const odd of [[{ ...ready, name: "" }], [{ ...ready, state: "loading" }], [{ ...ready, tools: "many" }], [null]]) {
      answer = () => ({ tag: "mcp_servers", data: odd });
      await click(h, root, "Check again");
      expect(group()?.textContent).toContain("The app could not read the tool servers Aether reported.");
    }
    answer = () => ({ tag: "mcp_servers", data: [ready] });
    await click(h, root, "Check again");
    expect(group()).toBeNull();
  });

  test("once Aether answers, the overview leads to starting a task", async () => {
    const h = current = await connected({ role: "local", desktop: setupDesktop(() => null).bridge });
    const root = await h.mount(<Welcome />);
    expect(root.querySelector('a[href="#/activity/start"]')?.textContent).toBe("Start a task");
  });
});

describe("SetupScreen", () => {
  test("one click runs the steps; a missing tool says how to install it and checks again", async () => {
    let archonFound = false;
    const desktop = setupDesktop((step) => {
      if (step === "tools") return { step, tools: [{ name: "aether", found: true, version: "1.3.0" }, { name: "archon", found: archonFound }] };
      throw new Error("Archon's tools could not be connected: archon exited with status 2");
    });
    const h = current = await connected({ role: "local", desktop: desktop.bridge });
    const root = await h.mount(<SetupScreen />);
    expect(root.querySelectorAll(".su-step")).toHaveLength(4);
    await click(h, root, "Set up this computer");
    expect(desktop.calls).toEqual(["tools"]);
    const failed = root.querySelector('.su-step[data-state="failed"]');
    expect(failed?.textContent).toContain("Archon is not installed on this computer yet.");
    await click(h, root, "How to install Archon");
    expect(desktop.openExternal).toHaveBeenCalledWith("https://github.com/mengsig/archon#install");

    archonFound = true;
    await click(h, root, "Check again");
    expect(desktop.calls).toEqual(["tools", "tools", "connect"]);
    expect(root.querySelector('.su-step[data-state="done"]')?.textContent).toContain("Aether 1.3.0 and Archon found");
    expect(root.querySelector('.su-step[data-state="failed"]')?.textContent).toContain("archon exited with status 2");
    await click(h, root, "Try again");
    expect(desktop.calls).toEqual(["tools", "tools", "connect", "connect"]);
  });

  test("a started setup goes on when its screen is left, reports its end once, and shows it on return", async () => {
    let fail: (error: Error) => void = () => undefined;
    const desktop = setupDesktop((step) => {
      if (step === "tools") return { step, tools: [{ name: "aether", found: true, version: "1.3.0" }, { name: "archon", found: true }] };
      return new Promise((_, reject) => { fail = reject; });
    });
    const h = current = await connected({ role: "local", desktop: desktop.bridge });
    let show: (shown: boolean) => void = () => undefined;
    function Away() {
      const [shown, setShown] = useState(true);
      useEffect(() => { show = setShown; }, []);
      return shown ? <SetupScreen /> : <p>Somewhere else</p>;
    }
    const root = await h.mount(<Away />);
    const visit = async (shown: boolean) => { act(() => show(shown)); await h.settle(); };
    await click(h, root, "Set up this computer");
    expect(root.querySelector('.su-step[data-state="running"]')?.textContent).toContain("Connect Archon's tools to Aether");

    await visit(false);
    expect(root.querySelector(".su-step")).toBeNull();
    fail(new Error("Archon's tools could not be connected: archon exited with status 2"));
    await h.settle();
    const stopped = () => h.app.store.get().reports.filter((r) => r.message.startsWith("Setup stopped"));
    expect(stopped()).toEqual([expect.objectContaining({ tone: "danger", message: "Setup stopped: Archon's tools could not be connected: archon exited with status 2" })]);

    await visit(true);
    expect(root.querySelector('.su-step[data-state="done"]')?.textContent).toContain("Aether 1.3.0 and Archon found");
    expect(root.querySelector('.su-step[data-state="failed"]')?.textContent).toContain("archon exited with status 2");
    await visit(false);
    await visit(true);
    expect(stopped()).toHaveLength(1);
    await click(h, root, "Try again");
    expect(desktop.calls).toEqual(["tools", "connect", "connect"]);
  });

  test("a browser is told setup happens in the desktop app", async () => {
    const h = current = await connected({ role: "operator" });
    const root = await h.mount(<SetupScreen />);
    expect(root.textContent).toContain("Setup runs in the desktop app");
    expect(root.querySelector("button")).toBeNull();
  });
});
