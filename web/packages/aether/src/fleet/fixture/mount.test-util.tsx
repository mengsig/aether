// Mounts a screen under a real core app whose store commits only when a test runs a frame (happy-dom).

import { AppProvider, createApp, type App } from "@aether-ui/core";
import { ThemeProvider } from "@aether-ui/ui";
import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { invariant } from "../invariant.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export interface Harness {
  readonly app: App;
  /** Commits pending store updates, as the next animation frame would. */
  frame(): Promise<void>;
  mount(node: ReactNode): Promise<HTMLElement>;
}

export function harness(): Harness {
  const frames: (() => void)[] = [];
  const memory = new Map<string, unknown>();
  const app = createApp(
    { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
    { scheduler: (flush) => frames.push(flush) },
  );
  const frame = async () => act(async () => { for (const flush of frames.splice(0)) flush(); });
  const mount = async (node: ReactNode) => {
    document.documentElement.lang = "en";
    const container = document.createElement("main");
    document.body.append(container);
    await frame();
    await act(async () => createRoot(container).render(
      <AppProvider app={app}><ThemeProvider initialPreference="light">{node}</ThemeProvider></AppProvider>,
    ));
    invariant(container.isConnected, "the screen mounted into the document");
    return container;
  };
  return { app, frame, mount };
}
