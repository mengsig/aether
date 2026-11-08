// Mounts the overview under a real core app whose reads answer from the seeded world and whose
// store commits only when a test runs a frame (happy-dom). Every read sent is logged.

import { AppProvider, applyRead, createApp, failRead, markRequested, type App } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { ThemeProvider } from "@aether-ui/ui";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { invariant } from "../invariant.ts";
import { LEARNER, SESSIONS } from "./world.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** A reply's data per command name; an Error answers that command with a failure. */
export type Replies = Readonly<Record<string, unknown>>;

export interface Harness {
  readonly app: App;
  /** `<conn> <cmd>` per reread and `<conn> noesis:<kind>` per Noesis read, in the order sent. */
  readonly sent: string[];
  /** Commits pending store updates, as the next animation frame would. */
  frame(): Promise<void>;
  mount(node: ReactNode): Promise<HTMLElement>;
  unmount(): Promise<void>;
}

function fakeReads(app: App, replies: Replies, sent: string[]): void {
  let issued = 0;
  app.reread = async (conn, key, command, tags) => {
    invariant(tags.length > 0, "a reread names the replies it accepts");
    issued += 1;
    const order = issued;
    sent.push(`${conn} ${String(command.cmd)}`);
    app.store.update((state) => markRequested(state, conn, key, order));
    const reply = replies[String(command.cmd)];
    if (reply === undefined || reply instanceof Error) {
      const message = reply instanceof Error ? reply.message : `no faux reply for ${String(command.cmd)}`;
      app.store.update((state) => failRead(state, conn, key, order, message));
      return;
    }
    app.store.update((state) => applyRead(state, conn, key, order, reply));
  };
  app.manager.readNoesis = async (conn, read) => {
    sent.push(`${conn} noesis:${read.kind}`);
    const failure = replies[`noesis:${read.kind}`];
    if (failure instanceof Error) throw failure;
    app.store.updateAether(conn, (state) => {
      if (read.kind === "learner") return { ...state, learn: { ...state.learn, reads: { ...state.learn.reads, learner: { value: LEARNER, ticket: 1 } } } };
      const sessions = Object.fromEntries(SESSIONS.sessions.map((session) => [session.id, session as unknown as aether.NoesisSession]));
      return { ...state, learn: { ...state.learn, sessions } };
    });
  };
}

export function harness(replies: Replies = {}): Harness {
  const frames: (() => void)[] = [];
  const memory = new Map<string, unknown>();
  const sent: string[] = [];
  const app = createApp(
    { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
    { scheduler: (flush) => frames.push(flush) },
  );
  fakeReads(app, replies, sent);
  let root: Root | null = null;
  let container: HTMLElement | null = null;
  const frame = async () => act(async () => { for (const flush of frames.splice(0)) flush(); });
  const mount = async (node: ReactNode) => {
    document.documentElement.lang = "en";
    container = document.createElement("main");
    document.body.append(container);
    await frame();
    const mounted = createRoot(container);
    root = mounted;
    await act(async () => mounted.render(<AppProvider app={app}><ThemeProvider initialPreference="light">{node}</ThemeProvider></AppProvider>));
    await frame();
    invariant(container.isConnected, "the screen mounted into the document");
    return container;
  };
  const unmount = async () => {
    const mounted = root;
    if (mounted !== null) await act(async () => mounted.unmount());
    container?.remove();
    root = null;
  };
  return { app, sent, frame, mount, unmount };
}
