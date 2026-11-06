// Pure derivations the overview renders: counts by state, hourly buckets, relative times, today's
// spend and the next step a tile offers when it has nothing to show yet. Nothing here reads a daemon.

import type { ArchonItem } from "@aether-ui/core";
import type { aether, archon, ConnectionInfo } from "@aether-ui/protocol";
import type { ChartPoint, ChartTone } from "./charts.ts";
import { invariant } from "./invariant.ts";

export const HOUR_MS = 3_600_000;
export const SPARK_HOURS = 12;
export const RECENT_EVENTS = 8;
export const RECENT_OUTCOMES = 5;

/** A reply checked at the boundary: its value, or why it cannot be shown. */
export type Parsed<T> = { readonly value: T } | { readonly problem: string };

export function parseTime(at: string | null | undefined): number | null {
  if (typeof at !== "string") return null;
  const time = Date.parse(at);
  return Number.isNaN(time) ? null : time;
}

/** Local midnight of the day `now` falls in. */
export function startOfDay(now: number): number {
  invariant(Number.isFinite(now), "now is a real time");
  const day = new Date(now);
  day.setHours(0, 0, 0, 0);
  invariant(day.getTime() <= now && now - day.getTime() < 25 * HOUR_MS, "a day starts at most a day before now");
  return day.getTime();
}

/** "just now", "5 min ago", "3 h ago", "2 d ago"; the raw text when it does not parse. */
export function relativeTime(at: string, now: number): string {
  const time = parseTime(at);
  if (time === null) return at;
  const minutes = Math.floor((now - time) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)} h ago`;
  return `${Math.floor(minutes / (24 * 60))} d ago`;
}

const hourLabel = (time: number): string => `${String(new Date(time).getHours()).padStart(2, "0")}:00`;

/** Events per clock hour for the last `hours` hours, the current hour last. */
export function hourlyBuckets(events: readonly aether.Event[], now: number, hours = SPARK_HOURS): ChartPoint[] {
  invariant(Number.isInteger(hours) && hours > 0 && hours <= 48, `a bucket count of ${hours} is a whole number of hours`);
  const current = Math.floor(now / HOUR_MS) * HOUR_MS;
  const first = current - (hours - 1) * HOUR_MS;
  const counts = new Array<number>(hours).fill(0);
  for (const event of events) {
    const time = parseTime(event.at);
    if (time === null || time < first || time >= current + HOUR_MS) continue;
    const index = Math.floor((time - first) / HOUR_MS);
    counts[index] = (counts[index] ?? 0) + 1;
  }
  const buckets = counts.map((value, i) => ({ label: hourLabel(first + i * HOUR_MS), value }));
  invariant(buckets.length === hours, "one bucket per hour");
  return buckets;
}

export function eventsSince(events: readonly aether.Event[], since: number): number {
  return events.reduce((count, event) => count + ((parseTime(event.at) ?? -Infinity) >= since ? 1 : 0), 0);
}

export interface AgentCounts { readonly total: number; readonly running: number; readonly waiting: number; readonly attention: number }

const WAITING: ReadonlySet<string> = new Set(["waiting", "ready"]);
const ATTENTION: ReadonlySet<string> = new Set(["blocked", "failed"]);

export function agentCounts(agents: readonly aether.Agent[]): AgentCounts {
  let running = 0;
  let waiting = 0;
  let attention = 0;
  for (const agent of agents) {
    const state: string = agent.status.state;
    invariant(typeof state === "string" && state !== "", `agent ${agent.id} has a status state`);
    if (state === "running") running += 1;
    else if (WAITING.has(state)) waiting += 1;
    else if (ATTENTION.has(state)) attention += 1;
  }
  invariant(running + waiting + attention <= agents.length, "each agent is counted at most once");
  return { total: agents.length, running, waiting, attention };
}

export interface NeedsYou { readonly questions: number; readonly approvals: number; readonly stopped: number; readonly archon: number; readonly total: number }

/** What waits on a person: Aether's attention snapshots (as the inbox counts them) and Archon's open questions. */
export function needsYou(attentions: readonly (aether.Attention | null)[], archonQuestions: number): NeedsYou {
  invariant(Number.isInteger(archonQuestions) && archonQuestions >= 0, "Archon's open questions are a count");
  let questions = 0;
  let approvals = 0;
  let stopped = 0;
  for (const attention of attentions) {
    if (attention === null) continue;
    questions += attention.questions.length;
    approvals += attention.approvals.filter((a) => (a.status as { state?: string }).state === "pending").length;
    stopped += attention.agents.filter((a) => a.kind !== "approval" && a.kind !== "question").length;
  }
  return { questions, approvals, stopped, archon: archonQuestions, total: questions + approvals + stopped + archonQuestions };
}

export interface QueueCounts { readonly queued: number; readonly running: number; readonly blocked: number; readonly total: number }

export function queueCounts(items: readonly ArchonItem[]): QueueCounts {
  let queued = 0;
  let running = 0;
  let blocked = 0;
  for (const item of items) {
    if (item.state === "queued" || item.state === "dispatching") queued += 1;
    else if (item.state === "running") running += 1;
    else if (item.state === "blocked") blocked += 1;
  }
  invariant(queued + running + blocked <= items.length, "each item is counted at most once");
  return { queued, running, blocked, total: items.length };
}

export type OutcomeKind = "succeeded" | "failed" | "cancelled";
export interface Outcome { readonly id: string; readonly title: string; readonly kind: OutcomeKind; readonly at: string }
const OUTCOMES: ReadonlySet<string> = new Set(["succeeded", "failed", "cancelled"]);

/** The newest finished items, newest first. */
export function recentOutcomes(items: readonly (readonly [string, ArchonItem])[], limit = RECENT_OUTCOMES): Outcome[] {
  const done: Outcome[] = [];
  for (const [id, item] of items) {
    if (!OUTCOMES.has(item.state)) continue;
    const summary = item.summary as Partial<archon.ItemSummary>;
    invariant(id !== "", "an item has an id");
    const title = typeof summary.title === "string" && summary.title !== "" ? summary.title : id;
    done.push({ id, title, kind: item.state as OutcomeKind, at: typeof summary.updated_at === "string" ? summary.updated_at : "" });
  }
  done.sort((a, b) => (parseTime(b.at) ?? 0) - (parseTime(a.at) ?? 0) || (a.id < b.id ? -1 : 1));
  return done.slice(0, limit);
}

export const tokensOf = (usage: aether.Usage): number => usage.input_tokens + usage.output_tokens;

export function formatUsd(value: number): string {
  invariant(Number.isFinite(value) && value >= 0, `spend ${value} is a nonnegative amount`);
  if (value > 0 && value < 0.01) return "<$0.01";
  return `$${value.toFixed(2)}`;
}

export function formatTokens(count: number): string {
  invariant(Number.isFinite(count) && count >= 0, `token count ${count} is not negative`);
  if (count < 1_000) return String(Math.round(count));
  if (count < 1_000_000) return `${(count / 1_000).toFixed(count < 10_000 ? 1 : 0)}k`;
  return `${(count / 1_000_000).toFixed(1)}M`;
}

export interface Spend {
  readonly usd: number;
  readonly tokens: number;
  /** False when some work was left out of the dollars (an engine reported no cost, a model has no price). */
  readonly complete: boolean;
  readonly byModel: readonly ChartPoint[];
  /** `breakdown`: the daemon's spend since midnight; `agents`: lifetime cost of agents active today. */
  readonly source: "breakdown" | "agents";
}

function checkUsage(usage: unknown, where: string): string | null {
  const record = usage as Partial<aether.Usage> | null;
  if (record === null || typeof record !== "object") return `${where} has no usage`;
  const amounts = [record.cost_usd, record.input_tokens, record.output_tokens];
  return amounts.every((n) => typeof n === "number" && Number.isFinite(n) && n >= 0) ? null : `${where} has a cost or token count that is not an amount`;
}

/** `usage_breakdown` replies grouped by model, checked at the boundary, summed across connections. */
export function spendFromBreakdowns(replies: readonly unknown[]): Parsed<Spend> {
  const byModel = new Map<string, number>();
  let usd = 0;
  let tokens = 0;
  let complete = true;
  for (const reply of replies) {
    const breakdown = reply as Partial<aether.UsageBreakdown> | null;
    if (breakdown === null || typeof breakdown !== "object" || !Array.isArray(breakdown.rows)) return { problem: "usage_breakdown has no rows" };
    const problem = checkUsage(breakdown.total, "usage_breakdown's total") ?? breakdown.rows.map((row) => checkUsage(row.usage, `row ${row.key}`)).find((p) => p !== null);
    if (problem) return { problem };
    const total = breakdown.total as aether.Usage;
    usd += total.cost_usd;
    tokens += tokensOf(total);
    complete &&= total.cost_complete && !breakdown.truncated;
    for (const row of breakdown.rows) byModel.set(row.key, (byModel.get(row.key) ?? 0) + row.usage.cost_usd);
  }
  return { value: { usd, tokens, complete, byModel: [...byModel].map(([label, value]) => ({ label, value })), source: "breakdown" } };
}

/** Without `usage_breakdown`: the lifetime cost of each agent that did something today, by model. */
export function spendFromAgents(agents: readonly aether.Agent[], dayStart: number): Spend {
  const byModel = new Map<string, number>();
  let usd = 0;
  let tokens = 0;
  let complete = true;
  for (const agent of agents) {
    const usage = agent.stats?.usage;
    const active = parseTime(agent.last_event_at) ?? parseTime(agent.updated_at) ?? -Infinity;
    if (usage === undefined || active < dayStart) continue;
    invariant(checkUsage(usage, agent.id) === null, `agent ${agent.id} has real usage`);
    const model = `${agent.model.provider}/${agent.model.model}`;
    usd += usage.cost_usd;
    tokens += tokensOf(usage);
    complete &&= usage.cost_complete;
    byModel.set(model, (byModel.get(model) ?? 0) + usage.cost_usd);
  }
  return { usd, tokens, complete, byModel: [...byModel].map(([label, value]) => ({ label, value })), source: "agents" };
}

// Fixed order; accent and info sit apart because their hues are close.
const SLICE_TONES: readonly ChartTone[] = ["accent", "warning", "info"];

/** The top models by spend in fixed tones, the rest folded into "Other". */
export function spendSlices(byModel: readonly ChartPoint[]): (ChartPoint & { tone: ChartTone })[] {
  const sorted = byModel.filter((point) => point.value > 0).sort((a, b) => b.value - a.value || (a.label < b.label ? -1 : 1));
  const top = sorted.length > SLICE_TONES.length + 1 ? sorted.slice(0, SLICE_TONES.length) : sorted.slice(0, SLICE_TONES.length + 1);
  const slices = top.map((point, i) => ({ ...point, tone: SLICE_TONES[i] ?? "neutral" as ChartTone }));
  const rest = sorted.slice(top.length).reduce((sum, point) => sum + point.value, 0);
  if (rest > 0) slices.push({ label: "Other", value: rest, tone: "neutral" });
  invariant(slices.length <= SLICE_TONES.length + 1, "the donut keeps at most four slices");
  return slices;
}

export interface ActivitySource { readonly conn: string; readonly events: readonly aether.Event[]; readonly agents: Readonly<Record<string, aether.Agent>> }
export interface ActivityRow { readonly key: string; readonly conn: string; readonly agent: string | null; readonly name: string; readonly label: string; readonly at: string }

/** `agent.turn_completed` reads "Turn completed", `approval.requested` "Approval requested". */
export function eventLabel(type: string): string {
  invariant(type !== "", "an event has a kind");
  const parts = type.split(".");
  const words = (parts[0] === "agent" && parts.length > 1 ? parts.slice(1) : parts).join(" ").replaceAll("_", " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** The newest events across connections, newest first, with the names of the agents they are about. */
export function recentActivity(sources: readonly ActivitySource[], limit = RECENT_EVENTS): ActivityRow[] {
  const rows: { readonly row: ActivityRow; readonly time: number; readonly seq: number }[] = [];
  for (const source of sources) {
    // Core appends events in sequence order, so each connection's newest are its last.
    for (const event of source.events.slice(-limit)) {
      invariant(Number.isSafeInteger(event.seq) && typeof event.kind.type === "string", "an event has a sequence and a kind");
      const agent = event.agent_id ?? null;
      const name = agent === null ? "Daemon" : source.agents[agent]?.name ?? agent;
      const row = { key: `${source.conn}/${event.seq}`, conn: source.conn, agent, name, label: eventLabel(event.kind.type), at: event.at };
      rows.push({ row, time: parseTime(event.at) ?? 0, seq: event.seq });
    }
  }
  rows.sort((a, b) => b.time - a.time || b.seq - a.seq);
  return rows.slice(0, limit).map((entry) => entry.row);
}

export type Mastery = aether.ConceptStateKind;
const MASTERY_SLICES: readonly { readonly state: Mastery; readonly label: string; readonly tone: ChartTone }[] = [
  { state: "demonstrated", label: "Demonstrated", tone: "success" },
  { state: "exposed", label: "Introduced", tone: "info" },
  { state: "fragile", label: "Fragile", tone: "warning" },
  { state: "misconception", label: "Misconception", tone: "danger" },
  { state: "unseen", label: "Not seen yet", tone: "neutral" },
];

/** The `noesis_learner` read as mastery slices, checked at the boundary. */
export function masterySlices(learner: unknown): Parsed<(ChartPoint & { tone: ChartTone })[]> {
  const concepts = (learner as { concepts?: unknown } | null)?.concepts;
  if (!Array.isArray(concepts)) return { problem: "noesis_learner carries no concepts" };
  const counts = new Map<string, number>();
  for (const concept of concepts as Partial<aether.ConceptState>[]) {
    if (!MASTERY_SLICES.some((slice) => slice.state === concept.state)) return { problem: `a concept has an unknown state ${String(concept.state)}` };
    counts.set(concept.state as string, (counts.get(concept.state as string) ?? 0) + 1);
  }
  const slices = MASTERY_SLICES.map(({ state, label, tone }) => ({ label, value: counts.get(state) ?? 0, tone }));
  invariant(slices.reduce((sum, slice) => sum + slice.value, 0) === concepts.length, "every concept is in one slice");
  return { value: slices };
}

/** The session with the newest activity, or null. */
export function currentSession(sessions: Readonly<Record<string, aether.NoesisSession>>): aether.NoesisSession | null {
  let newest: aether.NoesisSession | null = null;
  for (const session of Object.values(sessions)) {
    invariant(typeof session.id === "string" && session.id !== "", "a session has an id");
    if (newest === null || (parseTime(session.updated_at) ?? 0) > (parseTime(newest.updated_at) ?? 0)) newest = session;
  }
  return newest;
}

/** archond keeps `github_watcher` for the CLI on remote devices; the same test the GitHub overview makes. */
export const keptForCli = (problem: string): boolean => /not available from the web app/i.test(problem);

export interface WatcherSummary { readonly enabled: boolean; readonly repos: number; readonly dispatched: number; readonly ignored: number; readonly lastPoll: string | null; readonly errors: number }

/** Each archond's `github_watcher` view, checked at the boundary and summed. */
export function watcherSummary(views: readonly unknown[]): Parsed<WatcherSummary> {
  let summary: WatcherSummary = { enabled: false, repos: 0, dispatched: 0, ignored: 0, lastPoll: null, errors: 0 };
  for (const raw of views) {
    const view = raw as Partial<archon.GithubWatcherView> | null;
    if (view === null || typeof view !== "object" || typeof view.enabled !== "boolean" || !Array.isArray(view.repositories)) {
      return { problem: "github_watcher has no enabled flag or repositories" };
    }
    for (const repo of view.repositories) {
      if (!Number.isInteger(repo.dispatched) || !Number.isInteger(repo.ignored)) return { problem: `${repo.repository} has counts that are not whole numbers` };
      const polled = parseTime(repo.last_poll_at);
      const newer = polled !== null && polled > (parseTime(summary.lastPoll) ?? -Infinity);
      summary = {
        ...summary, repos: summary.repos + 1, dispatched: summary.dispatched + repo.dispatched, ignored: summary.ignored + repo.ignored,
        lastPoll: newer ? repo.last_poll_at ?? null : summary.lastPoll, errors: summary.errors + (repo.last_error ? 1 : 0),
      };
    }
    summary = { ...summary, enabled: summary.enabled || view.enabled };
  }
  return { value: summary };
}

/** The Aether connection Noesis is read from: a ready one serving it, else any serving it, else any Aether one. */
export function noesisConnection(connections: readonly ConnectionInfo[]): ConnectionInfo | undefined {
  const all = connections.filter((info) => info.daemon === "aether");
  const serving = all.filter((info) => info.features.includes("noesis"));
  return serving.find((info) => info.state === "ready") ?? serving[0] ?? all.find((info) => info.state === "ready") ?? all[0];
}
