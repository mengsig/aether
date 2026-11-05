// Where a GitHub page's data comes from: archond's cache and link reads (stored in core by useReread), the
// linked items' summaries and details (core keeps them current while shown) and Aether's agents.

import { selectAgents, selectConnections, selectItemDetail, selectItemLinks, selectItems, useApp, useReread, useSelector, type ReadEntry, type Stamped, type State } from "@aether-ui/core";
import { archon, type aether, type ConnectionInfo, type ConnId } from "@aether-ui/protocol";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { pageLinks } from "./join.ts";
import { githubRef, invariant, type PageRef } from "./ref.ts";

/** The most cached objects one read asks for (CONTRACTS.md §1.3). */
export const OBJECTS_LIMIT = 500;
/** Rereads follow archond's changes once they settle this long, so a burst reads once. */
export const SETTLE_MS = 1_000;

/** The archond connection the pages read: the ready one, else any, else none. */
export function useArchonConnection(): ConnectionInfo | undefined {
  const connections = useSelector(selectConnections);
  const archond = Object.values(connections).filter((info) => info.daemon === "archon");
  return archond.find((info) => info.state === "ready") ?? archond[0];
}

/** `value` as it stood when it last stayed put for `ms`; it starts at the mounted value. */
export function useSettled<T>(value: T, ms: number): T {
  invariant(ms > 0, "a settle window is positive");
  const [settled, setSettled] = useState(value);
  const latest = useRef(value);
  useEffect(() => {
    latest.current = value;
    if (Object.is(settled, value)) return;
    const timer = setTimeout(() => setSettled(latest.current), ms);
    return () => clearTimeout(timer);
  }, [value, ms, settled]);
  return settled;
}

/** How often a page judges again whether its cached object went stale. */
export const STALE_CHECK_MS = 60_000;

/** The time now, renewed every `ms` while mounted. */
export function useNow(ms: number): number {
  invariant(ms > 0, "a clock ticks at a positive interval");
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(timer);
  }, [ms]);
  return now;
}

/** Moves with every change archond streams, settled, so the page's reads follow links and imports. */
export function useChangeKey(conn: ConnId): number | null {
  const cursor = useSelector(useCallback((state: State) => state.archon[conn]?.changeCursor ?? null, [conn]));
  return useSettled(cursor, SETTLE_MS);
}

export interface GithubReads {
  /** The page's repository in archond's cache; undefined when it does not serve `github_import`. */
  readonly cache: ReadEntry | undefined;
  readonly links: readonly archon.GithubLink[];
  /** The `issue_items` reads, one per kind; empty when archond does not serve `github_links`. */
  readonly linkReads: readonly ReadEntry[];
}

const linksIn = (entry: ReadEntry | undefined): readonly archon.GithubLink[] => {
  const links = (entry?.value as { links?: unknown } | null | undefined)?.links;
  return Array.isArray(links) ? links as archon.GithubLink[] : [];
};

/** Reads the page's object and links, again after each settled change and on `reload`. */
export function useGithubReads(info: ConnectionInfo, page: PageRef, refresh: unknown): GithubReads {
  const has = (feature: string) => info.features.includes(feature);
  const scope = `${page.host}/${page.repo}`.toLowerCase();
  const objects = has("github_import") ? archon.listGithubObjects({ repo: page.repo, limit: OBJECTS_LIMIT }) : null;
  const linked = has("github_links");
  const cache = useReread(info.id, `d4:objects:${scope}`, objects, ["github_objects"], refresh);
  const issue = useReread(info.id, `d4:links:${scope}#${page.number}:issue`, linked ? archon.issueItems({ ref: githubRef(page, "issue") }) : null, ["issue_items"], refresh);
  const pull = useReread(info.id, `d4:links:${scope}#${page.number}:pr`, linked ? archon.issueItems({ ref: githubRef(page, "pull_request") }) : null, ["issue_items"], refresh);
  const links = useMemo(() => (linked ? pageLinks(page, [linksIn(issue), linksIn(pull)]) : []), [page, linked, issue, pull]);
  const linkReads = linked ? [issue, pull].filter((entry): entry is ReadEntry => entry !== undefined) : [];
  return { cache: objects ? cache : undefined, links, linkReads };
}

/** Where the reads stand: none answered yet, or failed with nothing to show, or some answered (maybe beside a failure). */
export function readStatus(entries: readonly (ReadEntry | undefined)[]): { readonly loading: boolean; readonly error: string | null; readonly answered: boolean } {
  invariant(entries.length > 0, "a page reads something");
  // `issued` is set only by a reply that was stored, so a value of null still counts as an answer.
  const answered = entries.some((entry) => entry !== undefined && entry.issued > 0);
  const error = entries.find((entry) => entry?.error)?.error ?? null;
  invariant(error === null || error.length > 0, "a failed read says why");
  return { loading: !answered && error === null, error, answered };
}

const sameRecord = <T,>(a: Readonly<Record<string, T>>, b: Readonly<Record<string, T>>): boolean => {
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((key) => Object.is(a[key], b[key]));
};

export interface ItemReads {
  readonly details: Readonly<Record<string, Stamped | undefined>>;
  readonly itemLinks: Readonly<Record<string, Stamped | undefined>>;
}

/** Shows the linked items to core, which reads and keeps their detail and links current, and reads them back. */
export function useLinkedItems(conn: ConnId, ready: boolean, ids: readonly string[]): ItemReads {
  const app = useApp();
  const text = ids.join(" ");
  useEffect(() => {
    if (!ready || text === "") return;
    app.manager.showItems(conn, text.split(" "));
    return () => app.manager.showItems(conn, []);
  }, [app, conn, ready, text]);
  const pick = useCallback((select: (conn: ConnId, item: string) => (state: State) => Stamped | undefined) => (state: State) =>
    Object.fromEntries(text.split(" ").filter(Boolean).map((id) => [id, select(conn, id)(state)])), [conn, text]);
  const details = useSelector(useMemo(() => pick(selectItemDetail), [pick]), sameRecord);
  const itemLinks = useSelector(useMemo(() => pick(selectItemLinks), [pick]), sameRecord);
  return { details, itemLinks };
}

export function useItems(conn: ConnId) {
  return useSelector(useMemo(() => selectItems(conn), [conn]));
}

export interface AgentFound { readonly conn: ConnId; readonly agent: aether.Agent }

export interface AetherSide {
  /** Whether any Aether connection is ready: without one the page names no agents. */
  readonly connected: boolean;
  readonly find: (agent: string) => AgentFound | null;
}

/** Every ready Aether connection's agents, so a stage's agent id finds its agent on whichever daemon ran it. */
export function useAether(): AetherSide {
  const select = useCallback((state: State) => {
    const ready = Object.values(state.connections).filter((info) => info.daemon === "aether" && info.state === "ready");
    return Object.fromEntries(ready.map((info) => [info.id, selectAgents(info.id)(state)]));
  }, []);
  const agents = useSelector(select, sameRecord);
  return useMemo(() => ({
    connected: Object.keys(agents).length > 0,
    find: (id: string) => {
      invariant(id.length > 0, "an agent is looked up by id");
      for (const [conn, byId] of Object.entries(agents)) {
        const agent = byId?.[id];
        if (agent) return { conn, agent };
      }
      return null;
    },
  }), [agents]);
}
