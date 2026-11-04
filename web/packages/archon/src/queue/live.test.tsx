// @vitest-environment happy-dom
// The store is the only source of truth: one sequence of change events and daemon replies, with the facts
// the queue and item screens show after each step, through remounts and unrelated activity.
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import ItemRoute from "../item/ItemScreen.tsx";
import type { World } from "./fixture/world.ts";
import { invariant } from "./model.ts";
import QueueRoute from "./QueueScreen.tsx";
import { archonHarness, type ArchonHarness } from "./test/harness.tsx";

type Json = Record<string, unknown>;

interface Scene {
  readonly h: ArchonHarness;
  queue: HTMLElement | null;
  item: HTMLElement | null;
}

interface RowFacts { readonly pill: string; readonly stage: string | null; readonly note: string | null; readonly spend: string | null; readonly pr: string | null }

interface Step {
  readonly name: string;
  readonly act: (scene: Scene) => Promise<void>;
  /** False checks the facts the moment the act ends, before any reread can land. */
  readonly settle?: boolean;
  readonly queue?: Readonly<Record<string, Partial<RowFacts>>>;
  /** Each item fact as "text | detail". */
  readonly item?: Readonly<Record<string, string>>;
}

function rowFacts(container: HTMLElement, id: string): RowFacts {
  const row = container.querySelector(`.queue-row[data-item="${id}"]`);
  invariant(row !== null, `the queue shows ${id}`);
  const text = (selector: string) => row.querySelector(selector)?.textContent ?? null;
  const pr = /PR #(\d+)/.exec(row.querySelector(".queue-row-side")?.textContent ?? "");
  return {
    pill: text(".ui-pill") ?? "", stage: /stage (\w+)/.exec(text(".queue-meta") ?? "")?.[1] ?? null,
    note: text(".queue-note"), spend: text(".queue-spend"), pr: pr ? `#${pr[1]}` : null,
  };
}

function itemFacts(container: HTMLElement): Record<string, string> {
  const cards = [...container.querySelectorAll(".item-fact")];
  invariant(cards.length === 3, `the item page states three facts, not ${cards.length}`);
  return Object.fromEntries(cards.map((card) => {
    const label = card.querySelector(".item-fact-label")?.textContent ?? "";
    const [, text = ""] = (card.getAttribute("aria-label") ?? "").split(": ");
    const detail = card.querySelector(".item-muted")?.textContent ?? "";
    return [label, detail ? `${text} | ${detail}` : text];
  }));
}

function entry(world: World, id: string) {
  const found = world.items.get(id);
  invariant(found !== undefined, `the demo world has ${id}`);
  return found;
}

/** Edits an item as archond would commit it; its change is told separately. */
function commit(world: World, id: string, summary: Json, detail: Json = {}): void {
  const found = entry(world, id);
  found.summary = { ...found.summary, ...summary, updated_at: new Date().toISOString() };
  found.detail = { ...found.detail, ...detail };
}

/** The item's one run, now running at `stage`. */
function activeRun(world: World, id: string, stage: string): Json[] {
  const [run, ...rest] = entry(world, id).detail.runs as Json[];
  invariant(run !== undefined && rest.length === 0, `${id} has one run`);
  return [{ ...run, state: { state: "running", stage_id: stage, attempt: 1 } }];
}

let scene: Scene | null = null;

async function open(s: Scene, item: string): Promise<void> {
  if (s.item) await s.h.unmount(s.item);
  s.item = await s.h.mount(<ItemRoute params={{ item }} />);
}

const stageChange = (item: string, run: string, stage: string): Json =>
  ({ subject: "stage", stage_run: `sr_${run}_${stage}_0`, run, item, stage_id: stage, attempt: 1, fan_index: 0, state: "running" });

const STEPS: readonly Step[] = [
  {
    name: "a change before the queue first mounts",
    act: async ({ h }) => {
      commit(h.world, "qi_rate_limit", { state: { state: "running", run: "run_rl" }, stage: "plan", waiting_reason: null, spent_usd: 0.25 });
      h.faux.tell("qi_rate_limit", { subject: "item", item: "qi_rate_limit", state: "running", stage: "plan" });
    },
  },
  {
    name: "the queue mounts on the store's data",
    settle: false,
    act: async (s) => { s.queue = await s.h.mount(<QueueRoute />); },
    queue: {
      qi_rate_limit: { pill: "Running", stage: "plan", note: null, spend: "$0.25" },
      qi_dark_mode: { pill: "Running", stage: "review", spend: "$3.18", pr: "#57" },
    },
  },
  {
    name: "the item page opens",
    act: (s) => open(s, "qi_dark_mode"),
    item: { Execution: "Running | stage review", "Pull request": "#57 · Draft | checks pending" },
  },
  {
    name: "a stage change review→docs",
    act: async ({ h }) => {
      commit(h.world, "qi_dark_mode", { stage: "docs" }, { runs: activeRun(h.world, "qi_dark_mode", "docs") });
      h.faux.tell("qi_dark_mode", stageChange("qi_dark_mode", "run_dark", "docs"));
    },
    queue: { qi_dark_mode: { stage: "docs" } },
    item: { Execution: "Running | stage docs" },
  },
  {
    name: "spend refreshed by a run change",
    act: async ({ h }) => {
      commit(h.world, "qi_dark_mode", { spent_usd: 4.5 }, { spent_usd: 4.5 });
      h.faux.tell("qi_dark_mode", { subject: "run", run: "run_dark", item: "qi_dark_mode", state: "running" });
    },
    queue: { qi_dark_mode: { spend: "$4.50", stage: "docs" } },
  },
  {
    name: "PR #57→#58 through a link change",
    act: async ({ h }) => {
      const pr = { host: "github.com", repo: "acme/site", number: 58, kind: "pull_request" };
      const found = entry(h.world, "qi_dark_mode");
      const links = found.links.links as Json[];
      found.links = { ...found.links, links: [...links, { item: "qi_dark_mode", ref: pr, relation: "delivered", source: "ship", created_at: new Date().toISOString() }] };
      commit(h.world, "qi_dark_mode", { pr: { ...pr, state: "open" }, pr_url: "https://github.com/acme/site/pull/58" });
      h.faux.tell("qi_dark_mode", { subject: "link", item: "qi_dark_mode", host: "github.com", repo: "acme/site", number: 58, relation: "delivered" });
    },
    queue: { qi_dark_mode: { pr: "#58" } },
    item: { "Pull request": "#58 · Open", Execution: "Running | stage docs" },
  },
  {
    name: "the gated item opens",
    act: (s) => open(s, "qi_pairing"),
    queue: { qi_pairing: { pill: "Needs you", stage: "plan" } },
    item: { Execution: "Needs you | stage plan · Waiting on your gate decision for the proposal" },
  },
  {
    name: "a question change resolves the gate",
    act: async ({ h }) => {
      const question = h.world.questions.find((q) => q.id === "q_review_gate");
      invariant(question !== undefined, "the gate has its question");
      Object.assign(question, { answer: "Approve", answered_at: new Date().toISOString() });
      commit(h.world, "qi_pairing", { stage: "build", waiting_reason: null, waiting_on: null },
        { waiting_reason: null, waiting_on: null, runs: activeRun(h.world, "qi_pairing", "build") });
      h.faux.tell("qi_pairing", { subject: "question", question: "q_review_gate", item: "qi_pairing", open: false });
    },
    queue: { qi_pairing: { pill: "Running", stage: "build", note: null } },
    item: { Execution: "Running | stage build" },
  },
  {
    name: "an unrelated item's stage change leaves the gate resolved",
    act: async ({ h }) => {
      commit(h.world, "qi_dark_mode", { stage: "ship" }, { runs: activeRun(h.world, "qi_dark_mode", "ship") });
      h.faux.tell("qi_dark_mode", stageChange("qi_dark_mode", "run_dark", "ship"));
    },
    queue: { qi_pairing: { pill: "Running", stage: "build", note: null }, qi_dark_mode: { stage: "ship", spend: "$4.50", pr: "#58" } },
    item: { Execution: "Running | stage build" },
  },
  {
    name: "navigate away and stay idle",
    act: async (s) => {
      invariant(s.queue !== null, "the queue is mounted");
      await s.h.unmount(s.queue);
      s.queue = null;
      await open(s, "qi_changelog");
      await s.h.tick(10_000);
    },
    item: { Execution: "Succeeded", "Pull request": "#52 · Open | review required · 6 checks passed" },
  },
  {
    name: "back on the queue, the latest data stays",
    settle: false,
    act: async (s) => {
      invariant(s.item !== null, "an item page is mounted");
      await s.h.unmount(s.item);
      s.item = null;
      s.queue = await s.h.mount(<QueueRoute />);
    },
    queue: { qi_dark_mode: { spend: "$4.50", stage: "ship", pr: "#58" }, qi_pairing: { pill: "Running", stage: "build" }, qi_rate_limit: { pill: "Running", stage: "plan" } },
  },
];

beforeEach(() => { vi.useFakeTimers({ now: Date.parse("2026-11-04T10:00:00Z") }); });
afterEach(() => {
  scene?.h.stop();
  scene = null;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

test("each step's facts come from the newest data in core's store", async () => {
  scene = { h: await archonHarness({ protocol: 24 }), queue: null, item: null };
  for (const step of STEPS) {
    await step.act(scene);
    // Core's summary reread waits 500 ms, a shown item's 250 ms; replies land on later turns.
    if (step.settle !== false) await scene.h.tick(1_500);
    const { queue, item } = scene;
    for (const [id, expected] of Object.entries(step.queue ?? {})) {
      invariant(queue !== null, `${step.name}: the queue is mounted`);
      expect({ step: step.name, ...rowFacts(queue, id) }).toMatchObject({ step: step.name, ...expected });
    }
    if (step.item) {
      invariant(item !== null, `${step.name}: an item page is mounted`);
      expect({ step: step.name, ...itemFacts(item) }).toMatchObject({ step: step.name, ...step.item });
    }
  }
});
