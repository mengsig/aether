// What the queue shows of an item: its live state, why it waits, and how the queue orders and filters it.
// The live state kind comes from core (change stream or protocol 23 refetch); the summary may be older.

import type { ArchonItem } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import type { Status } from "@aether-ui/ui";

export type StateKind = archon.QueueStateKind;

/** Throws when a programmer assumption fails; expected bad input is handled where it arrives. */
export function invariant(condition: unknown, message: string): asserts condition {
  if (!condition) throw new Error(`@aether-ui/archon: ${message}`);
}

const KINDS: readonly StateKind[] = ["queued", "dispatching", "running", "blocked", "succeeded", "failed", "cancelled"];
const ACTIVE: ReadonlySet<StateKind> = new Set(["queued", "dispatching", "running", "blocked"]);
const TERMINAL: ReadonlySet<StateKind> = new Set(["succeeded", "failed", "cancelled"]);
/** Waiting reasons only the operator can clear. */
const OPERATOR_REASONS: ReadonlySet<string> = new Set(["operator_gate", "dependency_failed", "blocked"]);
const PRIORITY_RANK: Readonly<Record<string, number>> = { critical: 0, high: 1, normal: 2, low: 3 };
export const PRIORITIES: readonly archon.Priority[] = ["critical", "high", "normal", "low"];

export interface QueueRow {
  readonly id: string;
  readonly title: string;
  readonly kind: StateKind;
  readonly priority: archon.Priority;
  readonly repo: string | null;
  readonly pipeline: string;
  readonly stage: string | null;
  /** The live waiting reason code, and the summary's words for it when they describe the same code. */
  readonly waitingCode: string | null;
  readonly waitingText: string | null;
  readonly waitsOnYou: boolean;
  readonly blockedReason: string | null;
  readonly spentUsd: number;
  readonly updatedAt: string;
  readonly dependencies: number;
  readonly brief: string;
  readonly issue: archon.GithubRef | null;
  readonly pr: archon.PrRef | null;
  readonly prUrl: string | null;
}

export function isStateKind(value: string): value is StateKind {
  return (KINDS as readonly string[]).includes(value);
}

export const isTerminal = (kind: StateKind): boolean => TERMINAL.has(kind);

/** One store item as the queue reads it; the live kind wins over the summary's older state. */
export function queueRow(id: string, item: ArchonItem): QueueRow {
  const summary = item.summary as unknown as archon.ItemSummary;
  invariant(typeof summary.title === "string", `item ${id} has a title`);
  invariant(isStateKind(item.state), `item ${id} has a known state, not ${item.state}`);
  const kind = item.state;
  const reason = summary.waiting_reason ?? null;
  const waitingCode = isTerminal(kind) ? null : item.waitingReason ?? null;
  const waitingText = waitingCode !== null && reason?.code === waitingCode ? reason.message : waitingCode === null ? null : reasonLabel(waitingCode);
  const blockedReason = kind === "blocked" && summary.state.state === "blocked" ? summary.state.reason : null;
  const waitingOn = !isTerminal(kind) && summary.waiting_on ? summary.waiting_on.prompt : null;
  return {
    id, title: summary.title, kind, priority: summary.priority, repo: summary.repo ?? null, pipeline: summary.switched_to ?? summary.pipeline,
    stage: item.stage ?? null, waitingCode, waitingText: waitingOn ?? waitingText,
    waitsOnYou: !isTerminal(kind) && (waitingOn !== null || kind === "blocked" || OPERATOR_REASONS.has(waitingCode ?? "")),
    blockedReason, spentUsd: summary.spent_usd ?? 0, updatedAt: summary.updated_at, dependencies: summary.dependencies?.length ?? 0,
    brief: summary.brief ?? "", issue: summary.issue ?? null, pr: summary.pr ?? null, prUrl: summary.pr_url ?? null,
  };
}

const REASON_LABELS: Readonly<Record<string, string>> = {
  aether_disconnected: "Aether is not connected", operator_gate: "Waiting on your gate decision",
  dependency_failed: "A dependency failed", dependency_waiting: "Waiting on a dependency", global_pause: "Archon is paused",
  low_disk: "Dispatch held: low disk space", total_budget: "Total budget reached", daily_budget: "Daily budget reached",
  repo_pause: "Its repository is paused", pipeline_pause: "Its pipeline is paused", capacity: "Waiting for capacity",
  next_in_line: "Next in line", draining: "Archon is draining", host_load: "Host load is high", blocked: "Blocked",
  provider_quota: "Provider quota exhausted",
};

export const reasonLabel = (code: string): string => REASON_LABELS[code] ?? code.replaceAll("_", " ");

/** The pill of an item's execution state; "Needs you" outranks a waiting state. */
export function statePill(kind: StateKind, waitsOnYou: boolean): { status: Status; label: string } {
  if (waitsOnYou && kind !== "blocked") return { status: "operator", label: "Needs you" };
  const labels: Record<StateKind, string> = {
    queued: "Queued", dispatching: "Starting", running: "Running", blocked: "Blocked", succeeded: "Succeeded", failed: "Failed", cancelled: "Cancelled",
  };
  return { status: kind, label: labels[kind] };
}

export interface QueueFilter {
  readonly id: string;
  readonly label: string;
  readonly test: (row: QueueRow) => boolean;
}

export const FILTERS: readonly QueueFilter[] = [
  { id: "active", label: "Active", test: (row) => ACTIVE.has(row.kind) },
  { id: "waiting", label: "Waiting on you", test: (row) => row.waitsOnYou },
  { id: "held", label: "Held up", test: (row) => !isTerminal(row.kind) && (row.kind === "blocked" || row.waitingCode !== null) },
  { id: "done", label: "Done", test: (row) => row.kind === "succeeded" || row.kind === "cancelled" },
  { id: "failed", label: "Failed", test: (row) => row.kind === "failed" },
  { id: "all", label: "All", test: () => true },
];

export interface QueueQuery {
  readonly filter: string;
  readonly repo: string;
  readonly pipeline: string;
  readonly priority: string;
  readonly text: string;
}

export const NO_QUERY: QueueQuery = { filter: "active", repo: "", pipeline: "", priority: "", text: "" };

/** Repository, pipeline, priority and text: everything but the state chip, whose counts use this scope. */
export function inScope(row: QueueRow, query: QueueQuery, repoNames: ReadonlyMap<string, string>): boolean {
  if (query.repo && row.repo !== query.repo) return false;
  if (query.pipeline && row.pipeline !== query.pipeline) return false;
  if (query.priority && row.priority !== query.priority) return false;
  const text = query.text.trim().toLowerCase();
  if (!text) return true;
  return [row.title, row.id, row.pipeline, row.brief, row.repo === null ? "" : repoNames.get(row.repo)]
    .some((field) => (field ?? "").toLowerCase().includes(text));
}

function rank(row: QueueRow): number {
  if (row.waitsOnYou) return 0;
  if (row.kind === "running" || row.kind === "dispatching") return 1;
  return isTerminal(row.kind) ? 3 : 2;
}

/** Needs-you first, then running, waiting and finished; open work by priority, then newest first. */
export function compareRows(a: QueueRow, b: QueueRow): number {
  const byRank = rank(a) - rank(b);
  if (byRank !== 0) return byRank;
  if (rank(a) < 3) {
    const byPriority = (PRIORITY_RANK[a.priority] ?? 2) - (PRIORITY_RANK[b.priority] ?? 2);
    if (byPriority !== 0) return byPriority;
  }
  return Date.parse(b.updatedAt) - Date.parse(a.updatedAt) || a.id.localeCompare(b.id);
}

export function usd(value: number | null | undefined): string {
  const amount = Number(value ?? 0);
  if (!Number.isFinite(amount)) return "—";
  if (amount > 0 && amount < 0.01) return "<$0.01";
  return `$${amount.toFixed(2)}`;
}

/** A short age such as "5 min" or "3 d"; an unreadable time says so. */
export function age(at: string, now: number): string {
  const then = Date.parse(at);
  if (!Number.isFinite(then)) return "unknown";
  const minutes = Math.max(0, Math.round((now - then) / 60_000));
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  return hours < 48 ? `${hours} h` : `${Math.round(hours / 24)} d`;
}

export function dateTime(at: string | null | undefined): string {
  const then = at ? Date.parse(at) : Number.NaN;
  return Number.isFinite(then) ? new Date(then).toLocaleString("en", { dateStyle: "medium", timeStyle: "short" }) : "—";
}

export const priorityLabel = (priority: string): string => priority.charAt(0).toUpperCase() + priority.slice(1);

/** An http(s) URL safe to link, or null; daemon text never becomes a javascript: link. */
export function safeHttpUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:" ? url.href : null;
  } catch {
    return null;
  }
}

/** The `data` of a reply, after checking its tag; a wrong tag is a daemon bug and is shown as one. */
export function replyOf<T>(reply: Record<string, unknown>, ...tags: readonly string[]): T {
  invariant(tags.length > 0, "a reply is read against at least one tag");
  if (!tags.includes(String(reply.reply))) throw new Error(`Expected a ${tags.join(" or ")} reply, got ${String(reply.reply)}`);
  return reply.data as T;
}
