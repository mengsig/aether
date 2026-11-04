// The archond connection the screens read, and replies they read on demand. The queue's items and an item's
// detail come from core's store; read models core does not keep (status, questions, plans) are read here.

import { selectConnections, useCapabilities, useCommand, useSelector } from "@aether-ui/core";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { useCallback, useEffect, useState } from "react";
import { invariant, replyOf } from "./model.ts";

/** The one archond connection (ARCHITECTURE.md §b): the ready one, else any, else none. */
export function useArchonConnection(): ConnectionInfo | undefined {
  const connections = useSelector(selectConnections);
  const archond = Object.values(connections).filter((info) => info.daemon === "archon");
  return archond.find((info) => info.state === "ready") ?? archond[0];
}

export interface ArchonAccess {
  readonly conn: string;
  readonly ready: boolean;
  /** Without `changes` the daemon speaks protocol 23 and the screens poll (CONTRACTS.md §5.3.5). */
  readonly live: boolean;
  readonly has: (feature: string) => boolean;
  /** Why a change control is disabled, or null when this device may operate. */
  readonly refusal: string | null;
}

export function useArchonAccess(info: ConnectionInfo): ArchonAccess {
  const caps = useCapabilities();
  const ready = info.state === "ready";
  const features = info.features;
  const has = useCallback((feature: string) => features.includes(feature), [features]);
  const operate = caps[`can.operate.${info.id}`] === true;
  const refusal = !ready ? "Archon is not connected" : operate ? null : "This device only observes Archon";
  return { conn: info.id, ready, live: features.includes("changes"), has, refusal };
}

export interface Read<T> {
  readonly data: T | undefined;
  readonly error: string | null;
  readonly reload: () => void;
}

/**
 * Reads `command` while the connection is ready and again whenever `key` changes; the last good
 * answer stays on screen through a failed reread, which shows its error beside it.
 */
export function useRead<T>(access: ArchonAccess, command: Record<string, unknown> | null, tags: readonly string[], key: unknown = 0): Read<T> {
  invariant(tags.length > 0, "a read names the reply it expects");
  const send = useCommand(access.conn);
  const [result, setResult] = useState<{ data?: T; dataFor: string | null; error: string | null; errorFor: string | null }>(
    { dataFor: null, error: null, errorFor: null });
  const [nonce, setNonce] = useState(0);
  const text = command === null ? null : JSON.stringify(command);
  const tagText = tags.join(" ");
  useEffect(() => {
    if (!access.ready || text === null) return;
    let current = true;
    // A reply with the wrong tag fails the read like a daemon error does.
    send(JSON.parse(text) as Record<string, unknown>).then((reply) => replyOf<T>(reply, ...tagText.split(" "))).then(
      (data) => { if (current) setResult({ data, dataFor: text, error: null, errorFor: null }); },
      (error: unknown) => { if (current) setResult((last) => ({ ...last, error: error instanceof Error ? error.message : String(error), errorFor: text })); },
    );
    return () => { current = false; };
  }, [access.ready, send, text, tagText, key, nonce]);
  const reload = useCallback(() => setNonce((n) => n + 1), []);
  // An answer to another command is not this command's answer.
  const data = result.dataFor === text ? result.data : undefined;
  return { data, error: result.errorFor === text ? result.error : null, reload };
}

/** Runs a change and reports how it went; core reports the failure too, and the daemon stays the gate. */
export function useChange(access: ArchonAccess): (command: Record<string, unknown>) => Promise<{ ok: true; reply: Record<string, unknown> } | { ok: false; error: string }> {
  const send = useCommand(access.conn);
  return useCallback(async (command) => {
    invariant(typeof command.cmd === "string", "a change names its command");
    try {
      return { ok: true, reply: await send(command) };
    } catch (error) {
      return { ok: false, error: error instanceof Error ? error.message : String(error) };
    }
  }, [send]);
}
