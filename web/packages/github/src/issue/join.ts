// Joins what core holds about one GitHub object: Archon's cached object, the items linked to it, their
// runs, outcomes and pull requests, and the Aether agents their stages ran on. Pure, so every state is testable.

import type { ArchonItem, Stamped } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { executionFact, outcomeText, pullRequestFact, RUN_LABELS, type StateFact } from "./facts.ts";
import { invariant, samePlace, type PageRef } from "./ref.ts";

/** A cached object this old may no longer be what GitHub says. */
export const STALE_MS = 30 * 60_000;

export interface RunView {
  readonly id: string;
  readonly status: StateFact["status"];
  readonly label: string;
  readonly startedAt: string;
}

export interface AgentUse {
  /** The Aether agent id a stage ran on. */
  readonly agent: string;
  readonly stages: readonly string[];
}

export interface ItemView {
  readonly id: string;
  readonly title: string;
  readonly relations: readonly archon.LinkRelation[];
  readonly execution: StateFact;
  readonly outcome: string | null;
  /** The item's own delivered pull request, when it is not the page's object. */
  readonly pr: StateFact | null;
  /** Null until core has read the item's detail. */
  readonly runs: readonly RunView[] | null;
  readonly agents: readonly AgentUse[];
}

export interface JoinInput {
  readonly page: PageRef;
  /** `list_github_objects` rows for the page's repository; null when not read. */
  readonly cached: readonly archon.GithubObject[] | null;
  /** `issue_items` links, of either kind. */
  readonly links: readonly archon.GithubLink[];
  readonly items: Readonly<Record<string, ArchonItem>>;
  readonly details: Readonly<Record<string, Stamped | undefined>>;
  readonly itemLinks: Readonly<Record<string, Stamped | undefined>>;
}

export interface PageView {
  readonly kind: archon.GithubKind | null;
  readonly object: archon.GithubObject | null;
  /** The PR state Archon recorded when it delivered this page's PR, for when the cache has not read it. */
  readonly recorded: archon.PrRef | null;
  readonly items: readonly ItemView[];
}

/** The page's links, one per (item, relation) whatever kind the read asked for, in the daemon's order. */
export function pageLinks(page: PageRef, replies: readonly (readonly archon.GithubLink[])[]): archon.GithubLink[] {
  const seen = new Set<string>();
  const links: archon.GithubLink[] = [];
  for (const link of replies.flat()) {
    invariant(typeof link.item === "string" && link.item.length > 0, "a GitHub link names its item");
    if (!samePlace(link.ref, page)) continue;
    const key = `${link.item} ${link.relation} ${link.ref.kind}`;
    if (seen.has(key)) continue;
    seen.add(key);
    links.push(link);
  }
  return links;
}

const objectsOf = (stamped: Stamped | undefined): readonly archon.GithubObject[] => {
  const objects = (stamped?.value as { objects?: unknown } | undefined)?.objects;
  return Array.isArray(objects) ? objects as archon.GithubObject[] : [];
};

/** The newest read of the page's object among the cache and the linked items' link replies. */
export function pageObject(input: JoinInput, itemIds: readonly string[]): archon.GithubObject | null {
  const candidates = [...(input.cached ?? []), ...itemIds.flatMap((id) => objectsOf(input.itemLinks[id]))].filter((object) => samePlace(object.ref, input.page));
  let newest: archon.GithubObject | null = null;
  for (const object of candidates) {
    invariant(typeof object.fetched_at === "string", "a cached GitHub object says when it was read");
    if (newest === null || Date.parse(object.fetched_at) > Date.parse(newest.fetched_at)) newest = object;
  }
  return newest;
}

const summaryOf = (item: ArchonItem | undefined): archon.ItemSummary | null => (item?.summary ?? null) as archon.ItemSummary | null;

/** Issue or pull request: from the object, else the links, else the items' own refs; null when nothing says. */
export function pageKind(input: JoinInput, object: archon.GithubObject | null, links: readonly archon.GithubLink[]): archon.GithubKind | null {
  if (object !== null) return object.ref.kind;
  const kinds = new Set(links.map((link) => link.ref.kind));
  for (const item of Object.values(input.items)) {
    const summary = summaryOf(item);
    if (summary?.issue && samePlace(summary.issue, input.page)) kinds.add(summary.issue.kind);
    if (summary?.pr && samePlace(summary.pr, input.page)) kinds.add("pull_request");
  }
  // One number names one object in a repository, so two kinds mean a confused record: say neither.
  return kinds.size === 1 ? [...kinds][0] ?? null : null;
}

function runsOf(detail: archon.ItemDetail): RunView[] {
  return detail.runs.map((run) => {
    const pill = RUN_LABELS[run.state.state];
    invariant(pill !== undefined, `a run state is known, not ${String(run.state.state)}`);
    return { id: run.id, status: pill.status, label: pill.label, startedAt: run.started_at };
  });
}

/** Each Aether agent the item's stages ran on, once, with the stages it ran, in first-seen order. */
export function agentsOf(detail: archon.ItemDetail): AgentUse[] {
  const byAgent = new Map<string, string[]>();
  for (const stage of detail.stages) {
    const agent = stage.aether_agent;
    if (!agent) continue;
    const stages = byAgent.get(agent) ?? [];
    if (!stages.includes(stage.stage_id)) stages.push(stage.stage_id);
    byAgent.set(agent, stages);
  }
  return [...byAgent].map(([agent, stages]) => ({ agent, stages }));
}

/** A linked item core has not read yet: a link names it before its summary arrives. */
const NOT_READ: StateFact = { label: "Execution", status: null, text: "Not read yet", detail: null };

function itemView(input: JoinInput, id: string, relations: readonly archon.LinkRelation[], kind: archon.GithubKind | null): ItemView {
  const stored = input.items[id];
  const summary = summaryOf(stored);
  const detail = (input.details[id]?.value ?? null) as archon.ItemDetail | null;
  invariant(detail === null || detail.item.id === id, `the detail of ${id} describes ${String(detail?.item.id)}`);
  const state = summary?.state ?? detail?.item.state ?? null;
  const kindOfState = stored?.state ?? state?.state ?? null;
  const whole = state !== null && state.state === kindOfState ? state : null;
  const pr = summary?.pr ?? null;
  const ownPr = pr !== null && !(kind === "pull_request" && samePlace(pr, input.page));
  const prObject = ownPr ? objectsOf(input.itemLinks[id]).find((object) => object.ref.kind === "pull_request" && samePlace(object.ref, pr)) ?? null : null;
  return {
    id, relations,
    title: summary?.title ?? detail?.item.title ?? id,
    execution: kindOfState === null ? NOT_READ : executionFact(kindOfState, stored?.stage ?? summary?.stage ?? null),
    outcome: outcomeText(whole),
    pr: ownPr ? pullRequestFact("Delivered PR", pr, prObject) : null,
    runs: detail ? runsOf(detail) : null,
    agents: detail ? agentsOf(detail) : [],
  };
}

export function joinPage(input: JoinInput): PageView {
  const relations = new Map<string, archon.LinkRelation[]>();
  for (const link of input.links) relations.set(link.item, [...(relations.get(link.item) ?? []), link.relation]);
  const ids = [...relations.keys()];
  const object = pageObject(input, ids);
  const kind = pageKind(input, object, input.links);
  const recorded = Object.values(input.items).map((item) => summaryOf(item)?.pr ?? null).find((pr) => pr !== null && samePlace(pr, input.page)) ?? null;
  const items = ids.map((id) => itemView(input, id, relations.get(id) ?? [], kind));
  invariant(new Set(items.map((item) => item.id)).size === items.length, "each linked item is listed once");
  return { kind, object, recorded: kind === "pull_request" ? recorded : null, items };
}

/** Whether the cached object is older than `STALE_MS` at `now`; an unparseable time counts as stale. */
export function isStale(object: archon.GithubObject, now: number): boolean {
  invariant(Number.isFinite(now), "staleness is judged at a real time");
  const fetched = Date.parse(object.fetched_at);
  return !Number.isFinite(fetched) || now - fetched > STALE_MS;
}
