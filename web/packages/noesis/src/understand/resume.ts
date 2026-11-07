// Finding the session an evidence source already started, so "Learn from this" can pick it up again.
// The key is the request itself (kind, connection, entity and a catch-up's day); the sessions the client
// started are remembered under it, and a session whose goal is the source's own goal matches as a fallback.

import { useApp } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { useCallback, useEffect, useMemo, useState } from "react";
import { errorText, useLearn, useNoesisRead } from "../learn/data.ts";
import { invariant, sortSessions } from "../learn/model.ts";
import type { UnderstandRequest } from "./bundle.ts";
import { putInStringMap, readStringMap } from "./string-map.ts";

export const SESSIONS_KEY = "noesis.understand.sessions";
export const VISITS_KEY = "noesis.catchup.visits";

/** One request names one source: the same pull request, or the same repository and first day. */
export function sourceKey(request: UnderstandRequest): string {
  return [request.kind, request.conn, request.id, request.since].filter((part) => part !== undefined).join("|");
}

/** The session remembered under the key if it still exists, else the newest one whose goal is `goal`. */
export function findResumable(sessions: readonly aether.NoesisSession[], remembered: string | undefined, goal: string): aether.NoesisSession | null {
  invariant(goal.trim().length > 0, "a source has a goal to match");
  const mapped = sessions.find((session) => session.id === remembered);
  return mapped ?? sortSessions(sessions.filter((session) => session.goal.trim() === goal.trim()))[0] ?? null;
}

export type Resumable =
  | { readonly state: "checking" }
  | { readonly state: "none" }
  | { readonly state: "found"; readonly session: aether.NoesisSession }
  /** The earlier sessions could not be checked; a fresh start is still offered, with the reason. */
  | { readonly state: "unknown"; readonly reason: string };

/** The session id kept under this source's key once the store answered, or why it could not be read. */
interface Remembered {
  readonly loaded: boolean;
  readonly id?: string | undefined;
  readonly error: string | null;
}

export function useResumable(conn: string, request: UnderstandRequest, goal: string): Resumable {
  const app = useApp();
  const key = sourceKey(request);
  const read = useNoesisRead(conn, { kind: "sessions" });
  const learn = useLearn(conn);
  const [remembered, setRemembered] = useState<Remembered>({ loaded: false, error: null });
  useEffect(() => {
    let live = true;
    readStringMap(app.host.storage, SESSIONS_KEY).then(
      (map) => { if (live) setRemembered({ loaded: true, id: map[key], error: null }); },
      (error: unknown) => { if (live) setRemembered({ loaded: false, error: errorText(error) }); },
    );
    return () => { live = false; };
  }, [app, key]);
  const listed = learn.reads.sessions !== undefined;
  const sessions = useMemo(() => Object.values(learn.sessions), [learn.sessions]);
  if (remembered.error !== null) return { state: "unknown", reason: remembered.error };
  if (read.error !== null && !listed) return { state: "unknown", reason: read.error };
  if (!listed || !remembered.loaded) return { state: "checking" };
  const session = findResumable(sessions, remembered.id, goal);
  return session === null ? { state: "none" } : { state: "found", session };
}

/** What starting a session records: the session under its source, and a catch-up's time for its repository. */
export function useRecordStart(request: UnderstandRequest): (sessionId: string) => void {
  const app = useApp();
  return useCallback((sessionId) => {
    invariant(sessionId.length > 0, "a started session has an id");
    const fail = (what: string) => (error: unknown) => app.store.report(`Could not remember ${what}: ${errorText(error)}`);
    putInStringMap(app.host.storage, SESSIONS_KEY, sourceKey(request), sessionId).catch(fail("this session"));
    if (request.kind !== "repository") return;
    putInStringMap(app.host.storage, VISITS_KEY, visitKey(request.conn, request.id), new Date().toISOString()).catch(fail("this catch-up"));
  }, [app, request]);
}

/** Where a repository's last catch-up is kept: per Archon connection and repository. */
export const visitKey = (conn: string, repo: string): string => `${conn}|${repo}`;
