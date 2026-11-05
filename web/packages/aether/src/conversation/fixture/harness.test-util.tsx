// Mounts a screen under a real core app connected to a faux daemon (happy-dom). The store commits
// only when a test runs a frame, so each step's screen is the one a browser would paint.

import { AppProvider, createApp, type App } from "@aether-ui/core";
import { EntityActionProvider, ThemeProvider } from "@aether-ui/ui";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { invariant } from "../../fleet/invariant.ts";
import { FauxDaemon, type FauxOptions } from "./faux-daemon.ts";

// The page's entity action menus need the shell's provider; these tests register no actions.
const failAction = (action: { id: string }, error: unknown) => { throw new Error(`action ${action.id} failed`, { cause: error }); };

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export interface Harness {
  readonly app: App;
  readonly daemon: FauxDaemon;
  readonly conn: string;
  /** Lets replies and timers run, then commits pending store updates as the next frame would. */
  settle(): Promise<void>;
  mount(node: ReactNode): Promise<HTMLElement>;
  unmount(): void;
}

export async function connected(options: FauxOptions): Promise<Harness> {
  const frames: (() => void)[] = [];
  const memory = new Map<string, unknown>();
  const daemon = new FauxDaemon(options);
  const app = createApp(
    { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
    { scheduler: (flush) => frames.push(flush), endpoints: [{ daemon: "aether", transport: daemon.transport, target: "faux://aetherd", label: "faux" }] },
  );
  let root: Root | null = null;
  const settle = async () => act(async () => {
    for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
    for (const flush of frames.splice(0)) flush();
  });
  await app.start();
  await settle();
  const conn = `aether:${options.instance}`;
  invariant(app.store.get().connections[conn]?.state === "ready", "the faux daemon's connection is ready");
  const mount = async (node: ReactNode) => {
    document.documentElement.lang = "en";
    const container = document.createElement("main");
    document.body.append(container);
    root = createRoot(container);
    const mounted = root;
    await act(async () => mounted.render(<AppProvider app={app}><ThemeProvider initialPreference="light"><EntityActionProvider actions={[]} capabilities={{}} onError={failAction}>{node}</EntityActionProvider></ThemeProvider></AppProvider>));
    await settle();
    return container;
  };
  const unmount = () => {
    act(() => root?.unmount());
    app.stop();
    document.body.innerHTML = "";
  };
  return { app, daemon, conn, settle, mount, unmount };
}
