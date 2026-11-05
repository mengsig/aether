// Graph worlds for the C2 tests, screenshots and the 2,000-node bench. The demo extends the C1 demo
// queue with two initiatives, typed edges, a blocked item and a recorded item history. Nothing names real work.

import RECORDED from "../../history/fixture/item-history.json";
import { demoWorld, type FauxItem, type World } from "../../queue/fixture/world.ts";
import type { GraphWorld } from "./faux-graph.ts";

type Json = Record<string, unknown>;

const T0 = Date.parse("2026-11-04T09:00:00Z");
const at = (minutes: number): string => new Date(T0 + minutes * 60_000).toISOString();

interface Spec {
  id: string; title: string; state: Json; minutes: number; initiative?: string; deps?: string[]; repo?: string; spent?: number;
  waiting?: { code: string; message: string }; stage?: string; priority?: string;
}

/** A queue item as the C1 faux keeps it: summary, detail, links and artifacts. */
export function fauxItem(spec: Spec): FauxItem {
  const summary: Json = {
    id: spec.id, title: spec.title, brief: `${spec.title}.`, state: spec.state, repo: spec.repo ?? "repo_api", pipeline: "adaptive",
    priority: spec.priority ?? "normal", created_at: at(spec.minutes - 20), updated_at: at(spec.minutes), dependencies: spec.deps ?? [], tags: [],
    model_plan: {}, spent_usd: spec.spent ?? 0, initiative: spec.initiative ?? null, stage: spec.stage ?? null,
    waiting_reason: spec.waiting ? { ...spec.waiting, next_action: "wait" } : null, waiting_on: null, issue: null, pr: null, pr_url: null,
  };
  const item = Object.fromEntries(Object.entries(summary).filter(([key]) => !["spent_usd", "stage", "waiting_reason", "waiting_on", "issue", "pr", "pr_url"].includes(key)));
  return { summary, detail: { item, runs: [], stages: [], gates: [], spent_usd: summary.spent_usd }, links: { links: [], objects: [] }, artifacts: [] };
}

const SYNC = "in_sync";
const PAIR = "in_pairing";

const DEMO_ITEMS: Spec[] = [
  { id: "qi_sync_schema", title: "Design the sync schema", state: { state: "succeeded", run: "run_s1" }, minutes: -40, initiative: SYNC, spent: 1.9 },
  { id: "qi_sync_store", title: "Store sync cursors", state: { state: "running", run: "run_s2" }, minutes: 30, initiative: SYNC, deps: ["qi_sync_schema"], stage: "build", spent: 2.2 },
  { id: "qi_sync_api", title: "Serve the sync endpoint", state: { state: "queued" }, minutes: 32, initiative: SYNC, deps: ["qi_sync_store"],
    waiting: { code: "dependency_waiting", message: "Waiting on Store sync cursors" } },
  { id: "qi_sync_client", title: "Resume sync in the client", state: { state: "queued" }, minutes: 33, initiative: SYNC, deps: ["qi_sync_api"], repo: "repo_site",
    waiting: { code: "dependency_waiting", message: "Waiting on Serve the sync endpoint" } },
  { id: "qi_sync_docs", title: "Document offline sync", state: { state: "queued" }, minutes: 34, initiative: SYNC, deps: ["qi_sync_client"], repo: "repo_site",
    waiting: { code: "dependency_waiting", message: "Waiting on Resume sync in the client" } },
  { id: "qi_sync_metrics", title: "Count sync conflicts", state: { state: "queued" }, minutes: 35, initiative: SYNC, deps: ["qi_sync_store"] },
  { id: "qi_pair_ui", title: "Pairing screen copy", state: { state: "blocked", reason: "The design review asked for a second pass on the copy" }, minutes: 36,
    initiative: PAIR, deps: ["qi_pairing"], repo: "repo_site", waiting: { code: "blocked", message: "Blocked" } },
  { id: "qi_pair_tests", title: "Pairing end-to-end tests", state: { state: "queued" }, minutes: 37, initiative: PAIR, deps: ["qi_pair_ui"], repo: "repo_site",
    waiting: { code: "dependency_waiting", message: "Waiting on Pairing screen copy" } },
];

const INITIATIVES: Json[] = [
  { id: SYNC, title: "Offline sync", state: "open", atomic: false, contract: "Every client resumes sync from its cursor after a disconnect.\nNo change is applied twice.", created_at: at(-60), updated_at: at(35) },
  { id: PAIR, title: "Faster pairing", state: "open", atomic: true, contract: "A new device pairs in under a minute with an eight-character code.", created_at: at(-10), updated_at: at(37) },
];

/** The C1 demo queue plus two initiatives; `qi_dark_mode` carries the recorded history. */
export function demoGraphWorld(): GraphWorld {
  const base: World = demoWorld();
  for (const spec of DEMO_ITEMS) base.items.set(spec.id, fauxItem(spec));
  const pairing = base.items.get("qi_pairing");
  if (pairing) pairing.summary = { ...pairing.summary, initiative: PAIR };
  const recorded = RECORDED.data;
  return {
    base, initiatives: INITIATIVES.map((entry) => ({ ...entry })),
    extraEdges: [{ from: "qi_sync_api", to: "qi_pair_tests", kind: "initiative_after" }],
    histories: new Map([[recorded.item, recorded.transitions.map((row) => ({ ...row }))]]),
  };
}

const STATES: readonly Json[] = [
  { state: "queued" }, { state: "running", run: "r" }, { state: "succeeded", run: "r" }, { state: "queued" }, { state: "failed", run: "r", error: "timed out" },
  { state: "blocked", reason: "waiting on review" }, { state: "dispatching" }, { state: "queued" },
];

/** `count` items in 40 initiatives, each after up to two earlier ones nearby, from a seeded generator. */
export function largeGraphWorld(count: number, seed = 11): GraphWorld {
  let state = seed >>> 0;
  const random = () => (state = (Math.imul(1664525, state) + 1013904223) >>> 0) / 4294967296;
  const base: World = { ...demoWorld(), items: new Map() };
  const id = (i: number) => `qi_${String(i).padStart(5, "0")}`;
  const initiatives: Json[] = Array.from({ length: 40 }, (_, n) => ({ id: `in_${n}`, title: `Initiative ${n + 1}`, state: "open", atomic: false, contract: "", created_at: at(0), updated_at: at(0) }));
  for (let i = 0; i < count; i++) {
    const deps = new Set<string>();
    for (let k = 0; k < 2 && i > 0; k++) if (random() < 0.85) deps.add(id(Math.max(0, i - 1 - Math.floor(random() * 30))));
    const spec: Spec = { id: id(i), title: `Task ${i + 1}`, state: STATES[i % STATES.length] ?? { state: "queued" }, minutes: i, deps: [...deps],
      ...(i % 7 === 0 ? {} : { initiative: `in_${Math.floor(i / (count / 40))}` }) };
    base.items.set(spec.id, fauxItem(spec));
  }
  return { base, initiatives, extraEdges: [], histories: new Map() };
}
