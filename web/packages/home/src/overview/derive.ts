// The overview's tiles from the store's slices: one view per tile, each loading, failed or ready.

import type { ReadEntry } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import type { ChartPoint, ChartTone } from "./charts.ts";
import { invariant } from "./invariant.ts";
import {
  agentCounts, checkUsage, currentSession, eventsSince, hourlyBuckets, HOUR_MS, masterySlices, needsYou, noesisConnection, queueCounts, recentActivity,
  recentOutcomes, spendFromAgents, spendFromBreakdowns, watcherSummary, type ActivityRow, type AgentCounts, type NeedsYou, type Outcome,
  type Parsed, type QueueCounts, type Spend, type WatcherSummary,
} from "./model.ts";
import { noesisFailure, USAGE_FEATURE, type HomeSources } from "./sources.ts";
import { noesisState, presence, type Facts, type NoesisState } from "./steps.ts";

/** A read the tile waits on: still loading, failed with nothing held, or a checked value. */
export type Loaded<T> = { readonly state: "loading" } | { readonly state: "problem"; readonly problem: string } | { readonly state: "ready"; readonly value: T };

/** Joins rereads: loading until every one answered, failed when any failed with nothing held. */
export function joinReads<T>(entries: readonly (ReadEntry | undefined)[], parse: (values: readonly unknown[]) => Parsed<T>): Loaded<T> {
  const failed = entries.find((entry) => entry !== undefined && entry.value === null && entry.error !== null);
  if (failed?.error) return { state: "problem", problem: failed.error };
  if (entries.some((entry) => entry === undefined || entry.value === null)) return { state: "loading" };
  const parsed = parse(entries.map((entry) => entry?.value));
  return "problem" in parsed ? { state: "problem", problem: parsed.problem } : { state: "ready", value: parsed.value };
}

export interface LearnView {
  readonly state: NoesisState;
  readonly conn: string | null;
  readonly session: aether.NoesisSession | null;
  readonly sessions: number;
  readonly mastery: Loaded<(ChartPoint & { tone: ChartTone })[]>;
}

export interface HomeView {
  readonly now: number;
  readonly facts: Facts;
  readonly agents: AgentCounts;
  readonly buckets: readonly ChartPoint[];
  readonly needs: NeedsYou;
  readonly questions: Loaded<number>;
  readonly queue: QueueCounts;
  readonly outcomes: readonly Outcome[];
  readonly spend: Loaded<Spend>;
  readonly lifetime: Loaded<Lifetime>;
  readonly activity: readonly ActivityRow[];
  readonly lastHour: number;
  readonly learn: LearnView;
  readonly github: Loaded<WatcherSummary>;
}

export interface Lifetime {
  readonly input: number;
  readonly output: number;
  readonly cacheRead: number;
  readonly cacheWrite: number;
  readonly usd: number;
  readonly complete: boolean;
  readonly estimated: number | null;
  readonly listPrice: boolean;
  readonly stale: boolean;
}

function lifetimeFromStatus(values: readonly unknown[]): Parsed<Lifetime> {
  const total: Lifetime = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, usd: 0, complete: true, estimated: null, listPrice: false, stale: false };
  let result = total;
  for (const value of values) {
    const status = value as Partial<aether.DaemonStatus> | null;
    if (!status || typeof status !== "object") return { problem: "Status has no usage" };
    if (status.unavailable !== undefined && !Array.isArray(status.unavailable)) return { problem: "Status unavailable sections are invalid" };
    if (status.unavailable?.some((section) => section?.section === "usage_total")) return { problem: "Lifetime usage is unavailable" };
    if (status.stale && !status.stale.read_at) return { problem: "Lifetime usage has not been read yet" };
    const problem = checkUsage(status.usage_total, "status usage_total");
    if (problem) return { problem };
    const usage = status.usage_total as aether.Usage;
    result = { input: result.input + usage.input_tokens, output: result.output + usage.output_tokens,
      cacheRead: result.cacheRead + usage.cache_read_tokens, cacheWrite: result.cacheWrite + usage.cache_write_tokens,
      usd: result.usd + usage.cost_usd, complete: result.complete && usage.cost_complete,
      estimated: usage.estimated_cost_usd == null ? result.estimated : (result.estimated ?? 0) + usage.estimated_cost_usd,
      listPrice: result.listPrice || usage.cost_basis === "list_price" || usage.cost_basis === "mixed", stale: result.stale || Boolean(status.stale) };
  }
  return { value: result };
}

function lifetimeView(sources: HomeSources): Loaded<Lifetime> {
  if (sources.aether.some((source) => source.info.state !== "ready")) return { state: "problem", problem: "An Aether connection is offline" };
  const entries = sources.aether.filter((source) => source.info.state === "ready").map((source) => source.lifetime);
  const failed = entries.find((entry) => entry?.error);
  if (failed?.error) return { state: "problem", problem: failed.error };
  return joinReads(entries, lifetimeFromStatus);
}

function countQuestions(values: readonly unknown[]): Parsed<number> {
  if (!values.every(Array.isArray)) return { problem: "list_questions did not answer with a list" };
  return { value: values.reduce<number>((sum, list) => sum + (list as unknown[]).length, 0) };
}

/** Today's spend: the daemons' own breakdown when every ready one serves it, else agents' cost fields. */
function spendView(sources: HomeSources, agents: readonly aether.Agent[], dayStart: number): Loaded<Spend> {
  const ready = sources.aether.filter((source) => source.info.state === "ready");
  if (ready.length === 0) return { state: "ready", value: spendFromAgents(agents, dayStart) };
  const spend: Loaded<Spend> = ready.every((source) => source.info.features.includes(USAGE_FEATURE))
    ? joinReads(ready.map((source) => source.spend), spendFromBreakdowns)
    : { state: "ready", value: spendFromAgents(agents, dayStart) };
  if (spend.state !== "ready" || spend.value.usd !== 0 || spend.value.tokens !== 0) return spend;
  const lifetime = lifetimeView(sources);
  if (lifetime.state === "loading") return lifetime;
  const lifetimeHasUsage = lifetime.state === "problem" || lifetime.value.stale || !lifetime.value.complete || lifetime.value.input > 0 || lifetime.value.output > 0 || lifetime.value.cacheRead > 0 || lifetime.value.cacheWrite > 0;
  const activeWithUsage = agents.some((agent) => {
    const at = Date.parse(agent.last_event_at ?? agent.updated_at);
    const usage = agent.stats?.usage;
    return at >= dayStart && usage !== undefined && (usage.input_tokens > 0 || usage.output_tokens > 0 || usage.cache_read_tokens > 0 || usage.cache_write_tokens > 0);
  });
  const updatedToday = ready.some(({ events }) => events.some((event) => event.kind.type === "agent.usage_updated" && Date.parse(event.at) >= dayStart));
  return activeWithUsage || updatedToday || lifetimeHasUsage ? { state: "ready", value: { ...spend.value, complete: false } } : spend;
}

function learnView(sources: HomeSources, failures: Readonly<Record<string, string>>): LearnView {
  const info = noesisConnection(sources.connections);
  const source = sources.aether.find((candidate) => candidate.info.id === info?.id);
  const state = noesisState(info, source?.noesis ?? null);
  if (source === undefined || state !== "on") return { state, conn: info?.id ?? null, session: null, sessions: 0, mastery: { state: "loading" } };
  const failure = noesisFailure(failures, source.info.id);
  const learner = source.learn.reads.learner;
  const parsed = learner === undefined ? null : masterySlices(learner.value);
  const mastery: Loaded<(ChartPoint & { tone: ChartTone })[]> = failure !== null && parsed === null ? { state: "problem", problem: failure }
    : parsed === null ? { state: "loading" } : "problem" in parsed ? { state: "problem", problem: parsed.problem } : { state: "ready", value: parsed.value };
  const sessions = Object.keys(source.learn.sessions).length;
  return { state, conn: source.info.id, session: currentSession(source.learn.sessions), sessions, mastery };
}

export function deriveHome(sources: HomeSources, now: number, dayStart: number, failures: Readonly<Record<string, string>>): HomeView {
  invariant(dayStart <= now, "the day started before now");
  const agents = sources.aether.flatMap((source) => Object.values(source.agents));
  const events = sources.aether.flatMap((source) => source.events);
  const items = sources.archon.flatMap((source) => Object.entries(source.items));
  const questions = joinReads(sources.archon.filter((s) => s.info.state === "ready").map((s) => s.questions), countQuestions);
  const github = joinReads(sources.archon.filter((s) => s.info.state === "ready").map((s) => s.watcher), watcherSummary);
  const learn = learnView(sources, failures);
  const facts: Facts = {
    aether: presence(sources.connections, "aether"), archon: presence(sources.connections, "archon"), agents: agents.length, events: events.length,
    items: items.length, noesis: learn.state, sessions: learn.sessions, watched: github.state === "ready" ? github.value.repos : 0,
  };
  const view: HomeView = {
    now, facts, agents: agentCounts(agents), buckets: hourlyBuckets(events, now),
    needs: needsYou(sources.aether.map((source) => source.attention), questions.state === "ready" ? questions.value : 0), questions,
    queue: queueCounts(items.map(([, item]) => item)), outcomes: recentOutcomes(items), spend: spendView(sources, agents, dayStart), lifetime: lifetimeView(sources),
    activity: recentActivity(sources.aether.map((source) => ({ conn: source.info.id, events: source.events, agents: source.agents }))),
    lastHour: eventsSince(events, now - HOUR_MS), learn, github,
  };
  invariant(view.buckets.reduce((sum, bucket) => sum + bucket.value, 0) <= events.length, "the hourly buckets count only held events");
  return view;
}
