// What every perf scenario shares: a real core app whose store commits only when a scenario runs a
// frame, counters of React commits and DOM mutations, timers that repeat, and the results file.

import { AppProvider, createApp, type App } from "@aether-ui/core";
import { EntityActionProvider, ThemeProvider } from "@aether-ui/ui";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { act, Profiler, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** perf/run.ts points each run at its own directory. */
export const OUT_DIR = process.env.PERF_OUT ?? path.join(import.meta.dirname, ".out", "latest");
/** Wall-time samples per measure; the median is the value, the spread is reported beside it. */
export const REPEATS = 7;

export interface Counters {
  /** React commits that rendered anything under the mounted screen. */
  commits: number;
}

export interface Mounted {
  readonly app: App;
  readonly container: HTMLElement;
  readonly counters: Counters;
  /** Commits pending store updates, as the next animation frame would. */
  frame(): Promise<void>;
  /** Runs `step` in act, then a frame. */
  step(step: () => void): Promise<void>;
  /** DOM mutation records delivered since the last call. */
  mutations(): Promise<number>;
  unmount(): void;
}

const noActions = (action: { id: string }, error: unknown) => { throw new Error(`action ${action.id} failed`, { cause: error }); };

/** A core app without endpoints: scenarios seed its store as core's sync would. */
export function perfApp(): { app: App; frames: (() => void)[] } {
  const frames: (() => void)[] = [];
  const memory = new Map<string, unknown>();
  const app = createApp(
    { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
    { scheduler: (flush) => frames.push(flush) },
  );
  return { app, frames };
}

export async function mount(app: App, frames: (() => void)[], node: ReactNode): Promise<Mounted> {
  const counters: Counters = { commits: 0 };
  const frame = async () => act(async () => { for (const flush of frames.splice(0)) flush(); });
  await frame();
  document.documentElement.lang = "en";
  const container = document.createElement("main");
  document.body.append(container);
  let mutated = 0;
  const observer = new MutationObserver((records) => { mutated += records.length; });
  observer.observe(container, { subtree: true, childList: true, attributes: true, characterData: true });
  const root: Root = createRoot(container);
  const onRender = () => { counters.commits += 1; };
  await act(async () => root.render(
    <Profiler id="perf" onRender={onRender}>
      <AppProvider app={app}><ThemeProvider initialPreference="light"><EntityActionProvider actions={[]} capabilities={{}} onError={noActions}>{node}</EntityActionProvider></ThemeProvider></AppProvider>
    </Profiler>,
  ));
  await frame();
  if (!container.isConnected || container.childElementCount === 0) throw new Error("the screen mounted nothing");
  const step = async (run: () => void) => {
    await act(async () => run());
    await frame();
  };
  const mutations = async () => {
    // happy-dom delivers records to the callback on a later task, never through takeRecords().
    await new Promise((resolve) => setTimeout(resolve, 0));
    const seen = mutated;
    mutated = 0;
    return seen;
  };
  const unmount = () => {
    observer.disconnect();
    act(() => root.unmount());
    container.remove();
  };
  return { app, container, counters, frame, step, mutations, unmount };
}

export interface Timing {
  readonly median: number;
  /** (max - min) / median over the samples: how far apart repeats land on this host. */
  readonly spread: number;
  readonly samples: readonly number[];
}

export function timing(samples: readonly number[]): Timing {
  if (samples.length === 0 || samples.some((sample) => !Number.isFinite(sample) || sample < 0)) throw new Error("a timing needs real samples");
  const sorted = [...samples].sort((a, b) => a - b);
  const median = sorted[Math.floor(sorted.length / 2)] ?? NaN;
  const spread = median > 0 ? ((sorted.at(-1) ?? 0) - (sorted[0] ?? 0)) / median : 0;
  return { median, spread, samples };
}

/** Times `run` `REPEATS` times after one warm-up; `setup` runs untimed before each. */
export async function timed(run: () => Promise<void>, setup: () => Promise<void> = async () => undefined): Promise<Timing> {
  const samples: number[] = [];
  for (let i = 0; i <= REPEATS; i += 1) {
    await setup();
    const started = performance.now();
    await run();
    const elapsed = performance.now() - started;
    if (i > 0) samples.push(elapsed);
  }
  return timing(samples);
}

/** Heap in use after a full collection; the perf suite runs node with --expose-gc. */
export function heapUsed(): number {
  const gc = (globalThis as { gc?: () => void }).gc;
  if (typeof gc !== "function") throw new Error("the perf suite needs node --expose-gc (perf/vitest.config.ts)");
  gc();
  gc();
  return process.memoryUsage().heapUsed;
}

export interface Measure { readonly value: number; readonly spread?: number; readonly samples?: readonly number[] }

/** Collects one scenario's measures and writes them to perf/.out/<scenario>.json. */
export class Results {
  private readonly measures = new Map<string, Measure>();
  private readonly scenario: string;

  constructor(scenario: string) {
    this.scenario = scenario;
  }

  count(name: string, value: number): void {
    if (!Number.isInteger(value) || value < 0) throw new Error(`${name}: a count is a whole number, not ${value}`);
    this.put(name, { value });
  }

  amount(name: string, value: number): void {
    if (!Number.isFinite(value) || value < 0) throw new Error(`${name}: a size is a real number, not ${value}`);
    this.put(name, { value: Math.round(value) });
  }

  time(name: string, value: Timing): void {
    this.put(name, { value: Number(value.median.toFixed(3)), spread: Number(value.spread.toFixed(3)), samples: value.samples.map((s) => Number(s.toFixed(3))) });
  }

  private put(name: string, measure: Measure): void {
    if (!name.startsWith(`${this.scenario}.`)) throw new Error(`measure ${name} belongs to scenario ${this.scenario}`);
    if (this.measures.has(name)) throw new Error(`measure ${name} is recorded twice`);
    this.measures.set(name, measure);
  }

  write(): void {
    if (this.measures.size === 0) throw new Error(`scenario ${this.scenario} recorded nothing`);
    mkdirSync(OUT_DIR, { recursive: true });
    writeFileSync(path.join(OUT_DIR, `${this.scenario}.json`), `${JSON.stringify(Object.fromEntries(this.measures), null, 2)}\n`);
  }
}
