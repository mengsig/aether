// The Noesis reads the learning screens ask core for, and how each reply, or a change's reply,
// lands in the store (CONTRACTS.md §6.2). A screen never keeps a reply of its own.

import type { aether } from "@aether-ui/protocol";
import { invariant, object, string, type Json } from "./guards.ts";
import { installRead, installSession, installSessionList, noesisKey, removeSession, type NoesisCache } from "./noesis-cache.ts";
import { replyObject } from "./replies.ts";
import type { AetherConnState } from "./store.ts";

export type NoesisReadRequest =
  | { readonly kind: "status" }
  | { readonly kind: "sessions"; readonly limit?: number }
  | { readonly kind: "session"; readonly id: string }
  | { readonly kind: "graph"; readonly query?: string; readonly limit?: number }
  | { readonly kind: "learner"; readonly limit?: number }
  | { readonly kind: "jobs" };

const REPLY_TAGS: Readonly<Record<NoesisReadRequest["kind"], string>> = {
  status: "noesis_status", sessions: "noesis_sessions", session: "noesis_session", graph: "noesis_graph", learner: "noesis_learner", jobs: "noesis_jobs",
};

/** The command a read sends. */
export function noesisReadCommand(read: NoesisReadRequest): Json {
  switch (read.kind) {
    case "status": return { cmd: "noesis_status" };
    case "sessions": return { cmd: "noesis_list_sessions", limit: read.limit ?? 200 };
    case "session": return { cmd: "noesis_get_session", session: string(read.id, "session id") };
    case "graph": return { cmd: "noesis_graph", limit: read.limit ?? 2000, ...(read.query ? { query: read.query } : {}) };
    case "learner": return { cmd: "noesis_learner", limit: read.limit ?? 1000 };
    case "jobs": return { cmd: "noesis_jobs" };
  }
}

/** The store key a read's reply is kept under. */
export function noesisReadKey(read: NoesisReadRequest): string {
  if (read.kind === "session") return noesisKey.session(read.id);
  if (read.kind === "graph") return read.query ? `graph?${read.query}` : noesisKey.graph;
  return noesisKey[read.kind];
}

/** Installs a read's reply, asked under `ticket`: whole, and never over a newer one. */
export function installNoesisRead(state: AetherConnState, read: NoesisReadRequest, reply: Json, ticket: number): AetherConnState {
  const data = replyObject(reply, REPLY_TAGS[read.kind]);
  if (read.kind === "status") return installStatus(state, data, ticket);
  if (read.kind === "session") {
    invariant(data.id === read.id, "noesis_get_session answers the session asked for");
    return withLearn(state, installSession(state.learn, data as unknown as aether.NoesisSession));
  }
  if (read.kind === "sessions") return withLearn(state, installSessionList(state.learn, data, ticket));
  return withLearn(state, installRead(state.learn, noesisReadKey(read), data, ticket));
}

/** A status read or a `noesis_configure` reply replaces the status unless a later ask already did. */
function installStatus(state: AetherConnState, status: Json, ticket: number): AetherConnState {
  invariant(typeof status.enabled === "boolean", "noesis_status carries enabled");
  const learn = installRead(state.learn, noesisKey.status, {}, ticket);
  if (learn === state.learn) return state;
  return { ...state, noesis: status, learn };
}

/** Lands what a Noesis change answered: a created session, a new status, a deletion. */
export function installNoesisChange(state: AetherConnState, command: Json, reply: Json, ticket: number): AetherConnState {
  const cmd = string(command.cmd, "command.cmd");
  if (cmd === "noesis_configure") return installStatus(state, replyObject(reply, "noesis_status"), ticket);
  if (cmd === "noesis_create_session") return withLearn(state, installSession(state.learn, replyObject(reply, "noesis_session") as unknown as aether.NoesisSession));
  if (cmd === "noesis_delete_session") return withLearn(state, removeSession(state.learn, string(command.session, "noesis_delete_session.session")));
  if (cmd === "noesis_delete_proposition" || cmd === "noesis_delete_source") return withLearn(state, staleGraph(state.learn));
  object(reply, "reply");
  return state;
}

function staleGraph(learn: NoesisCache): NoesisCache {
  return { ...learn, versions: { ...learn.versions, [noesisKey.graph]: (learn.versions[noesisKey.graph] ?? 0) + 1 } };
}

function withLearn(state: AetherConnState, learn: NoesisCache): AetherConnState {
  return learn === state.learn ? state : { ...state, learn };
}
