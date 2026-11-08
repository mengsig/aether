// Mounts a learning screen under a real core app connected to the faux Noesis daemon (happy-dom).
// Store updates commit only when a test settles, and timers advance only when a test says so.

import { AppProvider, createApp, drafts, type App } from "@aether-ui/core";
import type { DesktopBridge, HostAdapter } from "@aether-ui/protocol";
import { ThemeProvider } from "@aether-ui/ui";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { invariant } from "../model.ts";
import { FauxNoesis, type FauxNoesisOptions } from "./faux-noesis.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export const CONN = "aether:faux-noesis";

/** The test runner's timer control (vitest's `vi`), passed in: a package may not import vitest itself. */
export interface TestTimers {
  useFakeTimers(options: { toFake: ("setTimeout" | "clearTimeout" | "setInterval" | "clearInterval")[] }): unknown;
  advanceTimersByTimeAsync(ms: number): Promise<unknown>;
  useRealTimers(): unknown;
}

export interface Harness {
  readonly app: App;
  readonly daemon: FauxNoesis;
  /** Lets replies run and commits store updates as frames would, moving timers on by `ms` after each frame. */
  settle(ms?: number): Promise<void>;
  mount(node: ReactNode): Promise<HTMLElement>;
  /** Unmounts the screen and keeps the app, as leaving a screen and coming back does. */
  remount(node: ReactNode): Promise<HTMLElement>;
  unmount(): void;
}

/** On `ipc` the page is the desktop app's: its bridge runs privileged actions, as main would, on the faux daemon. */
function hostFor(options: FauxNoesisOptions, daemon: FauxNoesis, memory: Map<string, unknown>): HostAdapter {
  const storage = { get: async (key: string) => memory.get(key), set: async (key: string, value: unknown) => { memory.set(key, value); } };
  if (options.transport !== "ipc") return { kind: "browser", storage };
  const desktop: Partial<DesktopBridge> = {
    version: 1,
    getPreference: async () => undefined,
    connect: async () => "local:faux-main",
    close: async () => undefined,
    privileged: async (action) => { await daemon.gate; return daemon.runPrivileged(action); },
  };
  return { kind: "electron", storage, desktop: desktop as DesktopBridge };
}

export async function connected(options: FauxNoesisOptions, timers: TestTimers): Promise<Harness> {
  const frames: (() => void)[] = [];
  const memory = new Map<string, unknown>();
  const daemon = new FauxNoesis(options);
  const app = createApp(
    hostFor(options, daemon, memory),
    { scheduler: (flush) => frames.push(flush), endpoints: [{ daemon: "aether", transport: daemon.transport, target: "faux://aetherd", label: "faux" }] },
  );
  let root: Root | null = null;
  let faked = false;
  const settle = async (ms = 0) => act(async () => {
    for (let round = 0; round < 4; round += 1) {
      for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
      for (const flush of frames.splice(0)) flush();
      if (faked) await timers.advanceTimersByTimeAsync(ms);
    }
  });
  await app.start();
  await settle();
  invariant(app.store.get().connections[CONN]?.state === "ready", "the faux daemon's connection is ready");
  timers.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "setInterval", "clearInterval"] });
  faked = true;
  const mount = async (node: ReactNode) => {
    document.documentElement.lang = "en";
    const container = document.createElement("main");
    document.body.append(container);
    root = createRoot(container);
    const mounted = root;
    await act(async () => mounted.render(<AppProvider app={app}><ThemeProvider initialPreference="light">{node}</ThemeProvider></AppProvider>));
    await settle();
    return container;
  };
  const remount = async (node: ReactNode) => {
    act(() => root?.unmount());
    document.body.innerHTML = "";
    return mount(node);
  };
  const unmount = () => {
    act(() => root?.unmount());
    app.stop();
    timers.useRealTimers();
    restoreCanvas?.();
    restoreCanvas = null;
    document.body.innerHTML = "";
    location.hash = "";
    for (const key of drafts.keys()) drafts.clear(key);
  };
  return { app, daemon, settle, mount, remount, unmount };
}

/** happy-dom does not announce a hash change by itself; a browser fires `hashchange` after `navigate`. */
export async function hashChanged(harness: Harness): Promise<void> {
  await act(async () => { dispatchEvent(new HashChangeEvent("hashchange")); });
  await harness.settle();
}

let restoreCanvas: (() => void) | null = null;

/** happy-dom has no 2D canvas or ResizeObserver; the graph canvas draws into stubs, undone on unmount. */
export function stubCanvas(): void {
  invariant(restoreCanvas === null, "the canvas is stubbed once per test");
  const ctx = {
    fillStyle: "", strokeStyle: "", lineWidth: 1, font: "", textBaseline: "alphabetic",
    setTransform: () => undefined, clearRect: () => undefined, strokeRect: () => undefined, beginPath: () => undefined,
    moveTo: () => undefined, lineTo: () => undefined, bezierCurveTo: () => undefined, stroke: () => undefined,
    fillRect: () => undefined, fillText: () => undefined, measureText: (text: string) => ({ width: text.length * 7 }),
  };
  const canvas = HTMLCanvasElement.prototype;
  const getContext = Object.getOwnPropertyDescriptor(canvas, "getContext");
  const width = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientWidth");
  const height = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
  const observer = (globalThis as { ResizeObserver?: unknown }).ResizeObserver;
  Object.defineProperty(canvas, "getContext", { configurable: true, value: () => ctx });
  Object.defineProperty(HTMLElement.prototype, "clientWidth", { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 400 });
  (globalThis as { ResizeObserver?: unknown }).ResizeObserver = class {
    readonly callback: () => void;
    constructor(callback: () => void) { this.callback = callback; }
    observe() { this.callback(); }
    disconnect() { return undefined; }
  };
  restoreCanvas = () => {
    const put = (target: object, key: string, descriptor: PropertyDescriptor | undefined) =>
      descriptor ? Object.defineProperty(target, key, descriptor) : Reflect.deleteProperty(target, key);
    put(canvas, "getContext", getContext);
    put(HTMLElement.prototype, "clientWidth", width);
    put(HTMLElement.prototype, "clientHeight", height);
    (globalThis as { ResizeObserver?: unknown }).ResizeObserver = observer;
  };
}

/** The element a test knows is there. */
export function $(root: ParentNode, selector: string): HTMLElement {
  const found = root.querySelector<HTMLElement>(selector);
  invariant(found !== null, `an element matching ${selector}`);
  return found;
}

export function must<T>(value: T | null | undefined, what: string): T {
  invariant(value !== null && value !== undefined, `${what} is there`);
  return value;
}

export function byText(root: ParentNode, selector: string, text: string): HTMLElement {
  const found = [...root.querySelectorAll<HTMLElement>(selector)].find((element) => element.textContent?.includes(text));
  invariant(found !== undefined, `a ${selector} reading "${text}"`);
  return found;
}

export async function click(harness: Harness, element: Element): Promise<void> {
  await act(async () => { (element as HTMLElement).click(); });
  await harness.settle();
}

/** Types into a text control the way React hears it: the native setter, then an input event. */
export async function type(harness: Harness, element: Element, value: string): Promise<void> {
  invariant(element instanceof HTMLTextAreaElement || element instanceof HTMLInputElement, "typing goes into a text control");
  const proto = element instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  await act(async () => {
    Object.getOwnPropertyDescriptor(proto, "value")?.set?.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await harness.settle();
}
