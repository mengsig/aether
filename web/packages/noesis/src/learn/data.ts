// How the learning screens reach Noesis: the one Aether connection serving it, the gate that keeps
// every read off while Noesis is disabled, rereads into core's store and changes through useCommand.

import { noesisKey, selectConnections, selectLearn, selectNoesisStatus, useApp, useCapabilities, useCommand, useSelector, type NoesisCache, type NoesisReadRequest } from "@aether-ui/core";
import type { aether, ConnectionInfo } from "@aether-ui/protocol";
import { useCallback, useEffect, useMemo, useState } from "react";
import { invariant } from "./model.ts";

/** A burst of events naming one entity rereads it once, this long after the last. */
export const SETTLE_MS = 150;

/** The Aether connection Noesis is read from: a ready one that serves it, else any that serves it, else any Aether one. */
export function useNoesisConnection(): ConnectionInfo | undefined {
  const connections = useSelector(selectConnections);
  return useMemo(() => {
    const all = Object.values(connections).filter((info) => info.daemon === "aether");
    const serving = all.filter((info) => info.features.includes("noesis"));
    return serving.find((info) => info.state === "ready") ?? serving[0] ?? all.find((info) => info.state === "ready") ?? all[0];
  }, [connections]);
}

export type Gate =
  | { readonly state: "disconnected" }
  | { readonly state: "unsupported"; readonly conn: string }
  | { readonly state: "loading"; readonly conn: string }
  | { readonly state: "off" | "on"; readonly conn: string; readonly status: aether.NoesisStatus };

/** Whether Noesis is on, from the `noesis_status` core read at connect; deciding sends nothing. */
export function useGate(): Gate {
  const info = useNoesisConnection();
  const conn = info?.id ?? "";
  const status = useSelector(useMemo(() => selectNoesisStatus(conn), [conn])) as aether.NoesisStatus | null;
  if (info === undefined) return { state: "disconnected" };
  if (info.state === "ready" && !info.features.includes("noesis")) return { state: "unsupported", conn };
  if (status === null) return info.state === "ready" ? { state: "loading", conn } : { state: "disconnected" };
  return { state: status.enabled ? "on" : "off", conn, status };
}

export function useLearn(conn: string): NoesisCache {
  invariant(conn.length > 0, "learning data is read for a connection");
  return useSelector(useMemo(() => selectLearn(conn), [conn]));
}

/** The status key a read waits on: a query of the graph goes stale with the graph. */
function versionKey(read: NoesisReadRequest): string {
  return read.kind === "session" ? noesisKey.session(read.id) : noesisKey[read.kind];
}

export interface ReadState {
  /** Why the last reread failed; the store keeps the last good reply beside it. */
  readonly error: string | null;
  readonly reload: () => void;
}

/**
 * Asks core to reread `read` into the store on mount, after events made it stale and on reload.
 * Pass null to read nothing, as a screen behind the gate does.
 */
export function useNoesisRead(conn: string, read: NoesisReadRequest | null): ReadState {
  const app = useApp();
  const text = read === null ? null : JSON.stringify(read);
  const key = read === null ? null : versionKey(read);
  const version = useSelector(useCallback((state) => (key === null ? 0 : state.aether[conn]?.learn.versions[key] ?? 0), [conn, key]));
  const [nonce, setNonce] = useState(0);
  const [failure, setFailure] = useState<{ for: string; text: string } | null>(null);
  useEffect(() => {
    if (text === null) return;
    let live = true;
    const timer = setTimeout(() => {
      app.manager.readNoesis(conn, JSON.parse(text) as NoesisReadRequest).then(
        () => { if (live) setFailure(null); },
        (error: unknown) => { if (live) setFailure({ for: text, text: errorText(error) }); },
      );
    }, version === 0 && nonce === 0 ? 0 : SETTLE_MS);
    return () => { live = false; clearTimeout(timer); };
  }, [app, conn, text, version, nonce]);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  return { error: failure !== null && failure.for === text ? failure.text : null, reload };
}

export type ChangeResult = { readonly ok: true; readonly reply: Record<string, unknown> } | { readonly ok: false; readonly error: string };

/** Sends a Noesis change through useCommand and lands its reply in the store; the failure is returned to show. */
export function useNoesisChange(conn: string): (command: Record<string, unknown>) => Promise<ChangeResult> {
  const app = useApp();
  const send = useCommand(conn);
  return useCallback(async (command) => {
    invariant(typeof command.cmd === "string" && command.cmd.startsWith("noesis_"), "a Noesis change names a noesis_* command");
    try {
      const reply = await send(command);
      app.manager.noesisChanged(conn, command, reply);
      return { ok: true, reply };
    } catch (error) {
      return { ok: false, error: errorText(error) };
    }
  }, [app, conn, send]);
}

/** Why this device may not send an operate change, or null. */
export function useOperateRefusal(conn: string): string | null {
  const caps = useCapabilities();
  return caps[`can.operate.${conn}`] === true ? null : "This device only observes Aether";
}

/** Whether this device may write host-only settings (enabled, budgets, billing). */
export function useHostAdmin(conn: string): boolean {
  return useCapabilities()[`can.host_admin.${conn}`] === true;
}

export const errorText = (error: unknown): string => (error instanceof Error ? error.message : String(error));
