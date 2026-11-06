// What the overview reads from core: one selector over every connection that keeps the store's own
// references, and the few rereads it needs, each sent once when its connection becomes ready.

import { selectAgents, selectAttention, selectConnections, selectEvents, selectItems, selectLearn, selectNoesisStatus, selectRead, useApp, type ArchonItem, type NoesisCache, type NoesisReadRequest, type ReadEntry, type State } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { useEffect, useRef, useState } from "react";
import { invariant } from "./invariant.ts";
import { noesisConnection } from "./model.ts";

export const WATCHER_KEY = "home:github_watcher";
export const QUESTIONS_KEY = "home:questions";
export const spendKey = (dayStart: number): string => `home:spend:${dayStart}`;
export const USAGE_FEATURE = "usage_breakdown";

export interface AetherSource {
  readonly info: ConnectionInfo;
  readonly agents: Readonly<Record<string, aether.Agent>>;
  readonly events: readonly aether.Event[];
  readonly attention: aether.Attention | null;
  readonly noesis: Readonly<Record<string, unknown>> | null;
  readonly learn: NoesisCache;
  readonly spend: ReadEntry | undefined;
}

export interface ArchonSource {
  readonly info: ConnectionInfo;
  readonly items: Readonly<Record<string, ArchonItem>>;
  readonly watcher: ReadEntry | undefined;
  readonly questions: ReadEntry | undefined;
}

export interface HomeSources {
  readonly connections: readonly ConnectionInfo[];
  readonly aether: readonly AetherSource[];
  readonly archon: readonly ArchonSource[];
}

/** Every connection's slices, in connection id order; each field is the store's own reference. */
export function selectHomeSources(dayStart: number): (state: State) => HomeSources {
  const key = spendKey(dayStart);
  return (state) => {
    const connections = Object.values(selectConnections(state)).sort((a, b) => (a.id < b.id ? -1 : a.id > b.id ? 1 : 0));
    const aether: AetherSource[] = [];
    const archon: ArchonSource[] = [];
    for (const info of connections) {
      const conn = info.id;
      if (info.daemon === "aether") {
        aether.push({
          info, agents: selectAgents(conn)(state), events: selectEvents(conn)(state), attention: selectAttention(conn)(state),
          noesis: selectNoesisStatus(conn)(state), learn: selectLearn(conn)(state), spend: selectRead(conn, key)(state),
        });
      } else {
        archon.push({ info, items: selectItems(conn)(state), watcher: selectRead(conn, WATCHER_KEY)(state), questions: selectRead(conn, QUESTIONS_KEY)(state) });
      }
    }
    invariant(aether.length + archon.length === connections.length, "every connection is an Aether or an Archon one");
    return { connections, aether, archon };
  };
}

function sameFields<T extends object>(a: readonly T[], b: readonly T[]): boolean {
  if (a.length !== b.length) return false;
  return a.every((left, i) => {
    const right = b[i] as Record<string, unknown> | undefined;
    return right !== undefined && Object.entries(left).every(([field, value]) => Object.is(value, right[field]));
  });
}

/** Equal when every connection holds the very same slices, so the overview renders only on a real change. */
export function sameHomeSources(a: HomeSources, b: HomeSources): boolean {
  return sameFields(a.connections, b.connections) && sameFields(a.aether, b.aether) && sameFields(a.archon, b.archon);
}

export type HomeRead =
  | { readonly kind: "reread"; readonly conn: string; readonly key: string; readonly command: Readonly<Record<string, unknown>>; readonly tags: readonly string[] }
  | { readonly kind: "noesis"; readonly conn: string; readonly read: NoesisReadRequest };

const readId = (read: HomeRead): string => (read.kind === "reread" ? `${read.conn} ${read.key}` : `${read.conn} noesis:${read.read.kind}`);

/** The reads the overview needs now: only for ready connections, so a connection becoming ready adds its own. */
export function homeReads(sources: HomeSources, dayStart: number, noesisConn: string | null): HomeRead[] {
  const reads: HomeRead[] = [];
  for (const { info } of sources.archon) {
    if (info.state !== "ready") continue;
    reads.push({ kind: "reread", conn: info.id, key: WATCHER_KEY, command: { cmd: "github_watcher" }, tags: ["github_watcher"] });
    reads.push({ kind: "reread", conn: info.id, key: QUESTIONS_KEY, command: { cmd: "list_questions", open_only: true }, tags: ["questions"] });
  }
  for (const { info } of sources.aether) {
    if (info.state !== "ready" || !info.features.includes(USAGE_FEATURE)) continue;
    const command = { cmd: "usage_breakdown", by: "model", since: new Date(dayStart).toISOString() };
    reads.push({ kind: "reread", conn: info.id, key: spendKey(dayStart), command, tags: ["usage_breakdown"] });
  }
  if (noesisConn !== null) {
    reads.push({ kind: "noesis", conn: noesisConn, read: { kind: "sessions", limit: 20 } }, { kind: "noesis", conn: noesisConn, read: { kind: "learner" } });
  }
  invariant(new Set(reads.map(readId)).size === reads.length, "each read is asked for once");
  return reads;
}

const errorText = (error: unknown): string => (error instanceof Error ? error.message : String(error));

/** How often the overview rereads what no event keeps current, while its window is visible. */
export const HOME_REFRESH_MS = 120_000;

/** Counts the refreshes due so far: one every HOME_REFRESH_MS, skipped while the window is hidden. */
function useRefreshRound(): number {
  const [round, setRound] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => { if (document.visibilityState === "visible") setRound((n) => n + 1); }, HOME_REFRESH_MS);
    return () => clearInterval(timer);
  }, []);
  return round;
}

/**
 * Sends each read once per refresh round while it stays wanted: a read that drops out (its connection
 * went down) is sent again when it comes back. Core keeps reread failures in the store; Noesis failures
 * come back here.
 */
export function useHomeReads(reads: readonly HomeRead[]): Readonly<Record<string, string>> {
  const app = useApp();
  const round = useRefreshRound();
  const sent = useRef({ round: 0, ids: new Set<string>() });
  const [failures, setFailures] = useState<Readonly<Record<string, string>>>({});
  const text = JSON.stringify(reads);
  useEffect(() => {
    if (sent.current.round !== round) sent.current = { round, ids: new Set() };
    const wanted = JSON.parse(text) as HomeRead[];
    const ids = new Set(wanted.map(readId));
    for (const id of [...sent.current.ids]) if (!ids.has(id)) sent.current.ids.delete(id);
    for (const read of wanted) {
      const id = readId(read);
      if (sent.current.ids.has(id)) continue;
      sent.current.ids.add(id);
      if (read.kind === "reread") {
        void app.reread(read.conn, read.key, read.command as Parameters<typeof app.reread>[2], read.tags);
        continue;
      }
      app.manager.readNoesis(read.conn, read.read).then(
        () => setFailures((held) => (id in held ? omit(held, id) : held)),
        (error: unknown) => setFailures((held) => ({ ...held, [id]: errorText(error) })),
      );
    }
  }, [app, text, round]);
  return failures;
}

function omit(record: Readonly<Record<string, string>>, key: string): Readonly<Record<string, string>> {
  return Object.fromEntries(Object.entries(record).filter(([held]) => held !== key));
}

export const noesisFailure = (failures: Readonly<Record<string, string>>, conn: string): string | null =>
  failures[`${conn} noesis:learner`] ?? failures[`${conn} noesis:sessions`] ?? null;

/** The ready connection Noesis is on, whose sessions and mastery the overview reads; null when there is none. */
export function noesisReadConn(sources: HomeSources): string | null {
  const info = noesisConnection(sources.connections);
  if (info === undefined || info.state !== "ready" || !info.features.includes("noesis")) return null;
  const status = sources.aether.find((source) => source.info.id === info.id)?.noesis ?? null;
  return status !== null && status.enabled === true ? info.id : null;
}
