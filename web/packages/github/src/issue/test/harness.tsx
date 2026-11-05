// Mounts a GitHub page under a real core app connected to the faux archond and, when asked, a faux aetherd.
// Timers are fake, so a test moves time on purpose: core's frames, the settle window and the stale clock.

import { AppProvider, createApp, type App, type Endpoint } from "@aether-ui/core";
import { ThemeProvider } from "@aether-ui/ui";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { vi } from "vitest";
import GithubRoute from "../GithubScreen.tsx";
import { invariant } from "../ref.ts";
import { FauxAether, FauxArchon, type FauxArchonOptions } from "../fixture/faux.ts";
import { demoWorld, type GithubWorld } from "../fixture/world.ts";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

export const ARCHON = "archon:faux";
export const AETHER = "aether:studio";

export interface HarnessOptions extends FauxArchonOptions {
  readonly aether?: boolean;
  readonly world?: GithubWorld;
  /** Set up the faux archond (fail or hold a command) before the app connects. */
  readonly before?: (faux: FauxArchon) => void;
}

export interface Harness {
  readonly app: App;
  readonly faux: FauxArchon;
  tick(ms?: number): Promise<void>;
  until(check: () => boolean, what: string, limitMs?: number): Promise<void>;
  /** Mounts the page for `repo#number` (`owner/name`) and returns its container. */
  open(repo: string, number: number): Promise<HTMLElement>;
  stop(): void;
}

export async function harness(options: HarnessOptions = {}): Promise<Harness> {
  invariant(vi.isFakeTimers(), "the GitHub harness runs on fake timers");
  const frames: (() => void)[] = [];
  const world = options.world ?? demoWorld();
  const faux = new FauxArchon(world, options);
  options.before?.(faux);
  const endpoints: Endpoint[] = [{ daemon: "archon", transport: faux.transport, target: "wss://faux/archon/ws", label: "faux" }];
  if (options.aether) endpoints.push({ daemon: "aether", transport: new FauxAether(world.agents).transport, target: "faux-aetherd", label: "studio" });
  const app = createApp({ kind: "browser", storage: { get: async () => undefined, set: async () => {} } }, { scheduler: (flush) => frames.push(flush), endpoints });
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
  await until(() => app.store.get().connections[ARCHON]?.state === "ready", "the faux archond to be ready");
  if (options.aether) await until(() => app.store.get().connections[AETHER]?.state === "ready", "the faux aetherd to be ready");
  const open = async (repo: string, number: number) => {
    const [owner, name] = repo.split("/");
    invariant(owner && name, `${repo} is owner/name`);
    document.documentElement.lang = "en";
    const container = document.createElement("main");
    document.body.append(container);
    const root = createRoot(container);
    roots.push(root);
    const params = { host: "github.com", owner, repo: name, number: String(number) };
    await act(async () => root.render(<AppProvider app={app}><ThemeProvider initialPreference="light"><GithubRoute params={params} /></ThemeProvider></AppProvider>));
    await tick();
    return container;
  };
  const stop = () => {
    for (const root of roots.splice(0)) act(() => root.unmount());
    app.stop();
    document.body.innerHTML = "";
  };
  return { app, faux, tick, until, open, stop };
}

/** The aria-labels of the state cards under `scope`, as a reader hears them. */
export function facts(scope: ParentNode | null): (string | null)[] {
  invariant(scope !== null, "facts are read from a mounted part of the page");
  return [...scope.querySelectorAll(".gh-fact")].map((fact) => fact.getAttribute("aria-label"));
}

export function buttonNamed(scope: ParentNode, label: string): HTMLButtonElement {
  const found = [...scope.querySelectorAll("button")].find((button) => button.textContent?.trim() === label);
  invariant(found !== undefined, `a button named ${label}`);
  return found;
}

export async function click(h: Harness, element: HTMLElement): Promise<void> {
  await act(async () => { element.click(); });
  await h.tick();
}
