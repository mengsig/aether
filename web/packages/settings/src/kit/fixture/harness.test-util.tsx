// Mounts a B3 screen under a real core app connected to the scripted daemon (happy-dom). The store
// commits only when a test settles, so each step's screen is the one a browser would paint.

import { AppProvider, createApp, type App, type IdentityStore } from "@aether-ui/core";
import type { DesktopBridge, KeyValueStore } from "@aether-ui/protocol";
import { ThemeProvider } from "@aether-ui/ui";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { invariant } from "../invariant.ts";
import { ScriptedDaemon, type ScriptOptions } from "./daemon.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export interface Harness {
  readonly app: App;
  readonly daemon: ScriptedDaemon;
  readonly conn: string;
  settle(): Promise<void>;
  mount(node: ReactNode): Promise<HTMLElement>;
  unmount(): void;
}

export async function connected(options: ScriptOptions & {
  readonly desktop?: DesktopBridge; readonly identities?: IdentityStore; readonly storage?: KeyValueStore;
} = {}): Promise<Harness> {
  const frames: (() => void)[] = [];
  const daemon = new ScriptedDaemon(options);
  const storage = options.storage ?? { get: async () => undefined, set: async () => {} };
  const host = options.desktop ? { kind: "electron" as const, desktop: options.desktop, storage } : { kind: "browser" as const, storage };
  const app = createApp(host, {
    scheduler: (flush) => frames.push(flush),
    endpoints: [{ daemon: "aether", transport: daemon.transport, target: "faux-aetherd", label: "studio" }],
    ...(options.identities ? { identities: options.identities } : {}),
  });
  let root: Root | null = null;
  const settle = async () => act(async () => {
    for (let turn = 0; turn < 30; turn += 1) await Promise.resolve();
    for (const flush of frames.splice(0)) flush();
  });
  await app.start();
  await settle();
  const conn = `aether:${options.instance ?? "b3"}`;
  invariant(app.store.get().connections[conn]?.state === "ready", "the scripted daemon's connection is ready");
  const mount = async (node: ReactNode) => {
    document.documentElement.lang = "en";
    const container = document.createElement("main");
    document.body.append(container);
    root = createRoot(container);
    const mounted = root;
    await act(async () => mounted.render(<AppProvider app={app}><ThemeProvider initialPreference="light">{node}</ThemeProvider></AppProvider>));
    await settle();
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

/** Clicks the button whose visible text is `label`, failing when there is none or it is disabled. */
export async function click(h: Harness, root: ParentNode, label: string | RegExp): Promise<void> {
  const button = buttonNamed(root, label);
  invariant(!button.disabled, `the ${String(label)} button is enabled`);
  await act(async () => button.click());
  await h.settle();
}

export function buttonNamed(root: ParentNode, label: string | RegExp): HTMLButtonElement {
  const matches = [...root.querySelectorAll("button")].filter((b) => (typeof label === "string" ? b.textContent?.trim() === label : label.test(b.textContent ?? "")));
  invariant(matches.length > 0, `a button named ${String(label)}`);
  return matches[0] as HTMLButtonElement;
}

/** Types into the input or textarea labelled `label`, as React sees a user's typing. */
export async function type(h: Harness, root: ParentNode, label: string, value: string): Promise<void> {
  const labelled = [...root.querySelectorAll("label")].find((l) => l.textContent?.trim() === label);
  invariant(labelled !== undefined, `a field labelled ${label}`);
  const input = labelled.htmlFor ? root.querySelector(`[id="${labelled.htmlFor}"]`) : labelled.querySelector("input, textarea, select");
  invariant(input instanceof HTMLInputElement || input instanceof HTMLTextAreaElement || input instanceof HTMLSelectElement, `${label} labels an input`);
  const proto = Object.getPrototypeOf(input) as object;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  invariant(setter !== undefined, "the input has a value setter");
  await act(async () => {
    setter.call(input, value);
    input.dispatchEvent(new Event(input instanceof HTMLSelectElement ? "change" : "input", { bubbles: true }));
  });
  await h.settle();
}
