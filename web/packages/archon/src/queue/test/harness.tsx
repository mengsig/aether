// Mounts Archon screens under a real core app connected to a faux archond. Timers are fake, so tests move
// time on purpose: the protocol 23 fallback backoff, core's debounces and the screens' polling.

import { AppProvider, createApp, type App } from "@aether-ui/core";
import { EntityActionProvider, ThemeProvider } from "@aether-ui/ui";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { vi } from "vitest";
import { invariant } from "../model.ts";
import { FauxArchond, type FauxOptions } from "../fixture/faux-archond.ts";
import { demoWorld, type World } from "../fixture/world.ts";

// The page's entity action menus need the shell's provider; these tests register no actions.
const failAction = (action: { id: string }, error: unknown) => { throw new Error(`action ${action.id} failed`, { cause: error }); };

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export const CONN = "archon:faux";

export interface ArchonHarness {
  readonly app: App;
  readonly faux: FauxArchond;
  readonly world: World;
  /** Moves fake time on by `ms`, running core's timers, replies and the store's frames. */
  tick(ms?: number): Promise<void>;
  /** Ticks until `check` holds, failing with `what` after `limitMs` of fake time. */
  until(check: () => boolean, what: string, limitMs?: number): Promise<void>;
  mount(node: ReactNode): Promise<HTMLElement>;
  /** Unmounts the screen `mount` put in `container`, as navigating away does. */
  unmount(container: HTMLElement): Promise<void>;
  stop(): void;
}

export async function archonHarness(options: FauxOptions, world: World = demoWorld()): Promise<ArchonHarness> {
  invariant(vi.isFakeTimers(), "the Archon harness runs on fake timers");
  const frames: (() => void)[] = [];
  const memory = new Map<string, unknown>();
  const faux = new FauxArchond(world, options);
  const app = createApp(
    { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
    { scheduler: (flush) => frames.push(flush), endpoints: [{ daemon: "archon", transport: faux.transport, target: "wss://faux/archon/ws", label: "faux" }] },
  );
  const roots = new Map<HTMLElement, Root>();
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
  await until(() => app.store.get().connections[CONN]?.state === "ready", "the faux archond to be ready");
  await until(() => Object.keys(app.store.get().archon[CONN]?.items ?? {}).length > 0, "the queue summaries");
  const mount = async (node: ReactNode) => {
    document.documentElement.lang = "en";
    const container = document.createElement("main");
    document.body.append(container);
    const root = createRoot(container);
    roots.set(container, root);
    await act(async () => root.render(<AppProvider app={app}><ThemeProvider initialPreference="light"><EntityActionProvider actions={[]} capabilities={{}} onError={failAction}>{node}</EntityActionProvider></ThemeProvider></AppProvider>));
    await tick();
    invariant(container.isConnected, "the screen mounted into the document");
    return container;
  };
  const unmount = async (container: HTMLElement) => {
    const root = roots.get(container);
    invariant(root !== undefined, "only a mounted screen is unmounted");
    roots.delete(container);
    await act(async () => root.unmount());
    container.remove();
    await tick();
  };
  const stop = () => {
    for (const root of roots.values()) act(() => root.unmount());
    roots.clear();
    app.stop();
  };
  return { app, faux, world, tick, until, mount, unmount, stop };
}

/** The element `selector` names whose text is `text`, or a failure naming both. */
export function byText<T extends Element = HTMLElement>(container: ParentNode, selector: string, text: string | RegExp): T {
  const found = [...container.querySelectorAll<T>(selector)].find((node) => {
    const content = node.textContent?.trim() ?? "";
    return typeof text === "string" ? content === text : text.test(content);
  });
  if (!found) throw new Error(`no ${selector} with text ${String(text)}`);
  return found;
}

export async function click(h: ArchonHarness, element: Element): Promise<void> {
  invariant(element instanceof HTMLElement, "a click lands on an HTML element");
  await act(async () => { element.click(); });
  await h.tick();
}

/** Sets a form control's value the way React notices, then fires its event. */
export async function type(h: ArchonHarness, element: Element, value: string): Promise<void> {
  invariant(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement || element instanceof HTMLSelectElement, "typing lands on a form control");
  const proto = Object.getPrototypeOf(element) as object;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  invariant(setter !== undefined, "the control has a value setter");
  await act(async () => {
    setter.call(element, value);
    element.dispatchEvent(new Event(element instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
  });
  await h.tick();
}
