// @vitest-environment happy-dom
// Acceptance: with Noesis disabled every learning route shows the off state, and navigating all of
// them, settings included, sends nothing but `noesis_status`.

import { matchRoute } from "@aether-ui/core";
import type { RouteProps } from "@aether-ui/protocol";
import { act, createElement, useSyncExternalStore, type ComponentType, type ReactNode } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { byText, click, connected, hashChanged, stubCanvas, type Harness } from "./fixture/harness.test-util.tsx";
import { scenario } from "./fixture/scenario.ts";
import routes from "./routes.ts";

let harness: Harness | null = null;
afterEach(() => {
  harness?.unmount();
  harness = null;
});

const VISITS = ["#/learn", "#/learn/sessions/ns_raft", "#/learn/graph", "#/learn/me", "#/learn/graph?panel=settings", "#/learn?panel=settings"];

type Render = (params: RouteProps["params"]) => ReactNode;

/** Every learning route's screen, loaded the way the shell loads it. */
async function loadScreens(): Promise<Map<string, Render>> {
  const screens = new Map<string, Render>();
  for (const route of routes) {
    const Screen = ((await route.load()) as { default: ComponentType<RouteProps> }).default;
    screens.set(route.path, (params) => createElement(Screen, { params }));
  }
  expect(screens.size).toBe(4);
  return screens;
}

function subscribeHash(listener: () => void): () => void {
  addEventListener("hashchange", listener);
  return () => removeEventListener("hashchange", listener);
}

/** A minimal shell outlet: the screen of the route the hash names. */
function Outlet({ screens }: { screens: Map<string, Render> }) {
  const hash = useSyncExternalStore(subscribeHash, () => location.hash);
  const match = matchRoute(routes, hash);
  const render = match ? screens.get(match.route.path) : undefined;
  if (!match || !render) throw new Error(`no learning route for ${hash}`);
  return render(match.params);
}

async function go(h: Harness, hash: string): Promise<void> {
  await act(async () => {
    location.hash = hash;
    dispatchEvent(new HashChangeEvent("hashchange"));
  });
  await h.settle(60_000);
}

describe("Noesis off", () => {
  test("every learning route shows the off state and sends nothing but noesis_status", async () => {
    harness = await connected(scenario(false), vi);
    expect(harness.daemon.names()).toContain("noesis_status");
    const screens = await loadScreens();
    const before = harness.daemon.sent.length;
    location.hash = "#/learn";
    const root = await harness.mount(<Outlet screens={screens} />);
    for (const hash of VISITS) {
      await go(harness, hash);
      expect(root.querySelector("[data-testid=noesis-off]"), hash).not.toBeNull();
      expect(root.textContent, hash).toContain("Noesis is off");
    }
    expect(document.body.textContent).toContain("Turn on Noesis");
    const during = harness.daemon.names().slice(before);
    expect(during.length).toBeGreaterThan(0);
    expect(during.every((cmd) => cmd === "noesis_status"), `sent while off: ${during.join(", ")}`).toBe(true);
  });

  test("control: with Noesis on the same walk reads every screen's data, so the log above would catch a read", async () => {
    stubCanvas();
    harness = await connected(scenario(true), vi);
    const screens = await loadScreens();
    const before = harness.daemon.sent.length;
    location.hash = "#/learn";
    const root = await harness.mount(<Outlet screens={screens} />);
    for (const hash of VISITS) await go(harness, hash);
    expect(root.querySelector("[data-testid=noesis-off]")).toBeNull();
    const during = new Set(harness.daemon.names().slice(before));
    for (const cmd of ["noesis_list_sessions", "noesis_get_session", "noesis_graph", "noesis_learner"]) expect(during, cmd).toContain(cmd);
  });

  test("the off state's button opens settings, which read only noesis_status", async () => {
    harness = await connected(scenario(false), vi);
    const screens = await loadScreens();
    const before = harness.daemon.sent.length;
    location.hash = "#/learn/me";
    const root = await harness.mount(<Outlet screens={screens} />);
    await click(harness, byText(root, "button", "Open Noesis settings"));
    await hashChanged(harness);
    await harness.settle(1_000);
    expect(location.hash).toBe("#/learn/me?panel=settings");
    expect(document.body.textContent).toContain("Turn on Noesis");
    expect(harness.daemon.names().slice(before)).toEqual(["noesis_status"]);
  });
});
