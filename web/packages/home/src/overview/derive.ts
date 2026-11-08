// The overview's tiles from the store's slices: one view per tile, each loading, failed or ready.

import type { ReadEntry } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import type { ChartPoint, ChartTone } from "./charts.ts";
import { invariant } from "./invariant.ts";
import {
  agentCounts, currentSession, eventsSince, hourlyBuckets, HOUR_MS, masterySlices, needsYou, noesisConnection, queueCounts, recentActivity,
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
  readonly activity: readonly ActivityRow[];
  readonly lastHour: number;
  readonly learn: LearnView;
  readonly github: Loaded<WatcherSummary>;
}

function countQuestions(values: readonly unknown[]): Parsed<number> {
  if (!values.every(Array.isArray)) return { problem: "list_questions did not answer with a list" };
  return { value: values.reduce<number>((sum, list) => sum + (list as unknown[]).length, 0) };
}

/** Today's spend: the daemons' own breakdown when every ready one serves it, else agents' cost fields. */
function spendView(sources: HomeSources, agents: readonly aether.Agent[], dayStart: number): Loaded<Spend> {
  const ready = sources.aether.filter((source) => source.info.state === "ready");
  if (ready.length === 0) return { state: "ready", value: spendFromAgents(agents, dayStart) };
  if (!ready.every((source) => source.info.features.includes(USAGE_FEATURE))) return { state: "ready", value: spendFromAgents(agents, dayStart) };
  return joinReads(ready.map((source) => source.spend), spendFromBreakdowns);
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
    queue: queueCounts(items.map(([, item]) => item)), outcomes: recentOutcomes(items), spend: spendView(sources, agents, dayStart),
    activity: recentActivity(sources.aether.map((source) => ({ conn: source.info.id, events: source.events, agents: source.agents }))),
    lastHour: eventsSince(events, now - HOUR_MS), learn, github,
  };
  invariant(view.buckets.reduce((sum, bucket) => sum + bucket.value, 0) <= events.length, "the hourly buckets count only held events");
  return view;
}
