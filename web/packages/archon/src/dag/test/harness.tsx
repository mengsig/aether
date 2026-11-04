// Mounts C2 screens under a real core app connected to the C2 faux archond, on fake timers like the
// C1 harness: tests move time on purpose through core's debounces and the store's frames.

import { AppProvider, createApp, type App } from "@aether-ui/core";
import { ThemeProvider } from "@aether-ui/ui";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { vi } from "vitest";
import { invariant } from "../../queue/model.ts";
import { FauxGraphArchond, type GraphWorld } from "../fixture/faux-graph.ts";
import { demoGraphWorld } from "../fixture/graph-world.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export const CONN = "archon:faux";

export interface GraphHarness {
  readonly app: App;
  readonly faux: FauxGraphArchond;
  tick(ms?: number): Promise<void>;
  until(check: () => boolean, what: string, limitMs?: number): Promise<void>;
  mount(node: ReactNode): Promise<HTMLElement>;
  stop(): void;
}

export async function graphHarness(world: GraphWorld = demoGraphWorld(), role: "operator" | "observer" = "operator"): Promise<GraphHarness> {
  invariant(vi.isFakeTimers(), "the C2 harness runs on fake timers");
  const frames: (() => void)[] = [];
  const memory = new Map<string, unknown>();
  const faux = new FauxGraphArchond(world, { protocol: 24, role });
  const app = createApp(
    { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
    { scheduler: (flush) => frames.push(flush), endpoints: [{ daemon: "archon", transport: faux.transport, target: "wss://faux/archon/ws", label: "faux" }] },
  );
  const roots: Root[] = [];
  const tick = async (ms = 0) => act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
    for (const flush of frames.splice(0)) flush();
  });
  const until = async (check: () => boolean, what: string, limitMs = 5_000) => {
    for (let spent = 0; spent <= limitMs; spent += 50) {
      if (check()) return;
      await tick(50);
    }
    throw new Error(`timed out waiting for ${what}`);
  };
  await act(async () => { await app.start(); });
  await until(() => app.store.get().archon[CONN]?.graph != null, "the task graph in core's store");
  const mount = async (node: ReactNode) => {
    document.documentElement.lang = "en";
    const container = document.createElement("main");
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    await act(async () => root.render(<AppProvider app={app}><ThemeProvider initialPreference="light">{node}</ThemeProvider></AppProvider>));
    await tick();
    return container;
  };
  const stop = () => {
    for (const root of roots.splice(0)) act(() => root.unmount());
    app.stop();
    document.body.replaceChildren();
  };
  return { app, faux, tick, until, mount, stop };
}

/** Clicks `element` and lets the store's frames and React settle. */
export async function click(h: Pick<GraphHarness, "tick">, element: Element): Promise<void> {
  invariant(element instanceof HTMLElement, "a click lands on an HTML element");
  await act(async () => { element.click(); });
  await h.tick();
}

/** Sets an input's value the way React notices, then fires its input event. */
export async function type(h: Pick<GraphHarness, "tick">, element: Element, value: string): Promise<void> {
  invariant(element instanceof HTMLInputElement, "typing lands on an input");
  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  invariant(setter !== undefined, "an input has a value setter");
  await act(async () => {
    setter.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await h.tick();
}

/** happy-dom has no 2D canvas: a context that only counts its frames, and a sized element. */
export function stubCanvas(): { frames: () => number } {
  let frames = 0;
  const ctx = {
    fillStyle: "", strokeStyle: "", lineWidth: 1, font: "", textBaseline: "alphabetic",
    setTransform: () => undefined, clearRect: () => { frames += 1; }, strokeRect: () => undefined, beginPath: () => undefined,
    moveTo: () => undefined, lineTo: () => undefined, bezierCurveTo: () => undefined, stroke: () => undefined,
    fillRect: () => undefined, fillText: () => undefined, measureText: (text: string) => ({ width: text.length * 7 }),
  };
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(ctx as unknown as CanvasRenderingContext2D);
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(900);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(600);
  vi.stubGlobal("ResizeObserver", class {
    private readonly callback: () => void;
    constructor(callback: () => void) { this.callback = callback; }
    observe() { this.callback(); }
    disconnect() { return undefined; }
  });
  vi.stubGlobal("matchMedia", () => ({ matches: false, addEventListener: () => undefined, removeEventListener: () => undefined }));
  return { frames: () => frames };
}
