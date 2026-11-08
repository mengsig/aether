// Whether this connection serves Memory 2.0 and may send its host-only commands.

import { usePermission, useSelector, type CommandInput, type Permission } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { useCallback } from "react";
import { invariant } from "../fleet/invariant.ts";

export const MEMORY_V2 = "memory_v2";

/** True when the daemon's welcome listed `memory_v2`; an older daemon sees the screen as before. */
export function useMemoryV2(conn: ConnId): boolean {
  return useSelector(useCallback((state) => state.connections[conn]?.features.includes(MEMORY_V2) === true, [conn]));
}

/** Apply the host-standing check before offering a memory maintenance command. */
export function useHostGate(conn: ConnId, command: CommandInput): Permission {
  invariant(typeof (command as { cmd?: unknown }).cmd === "string", "a gated command names its cmd");
  return usePermission(conn, command);
}

/** The data of a command's reply, checked to be the reply `tag` the contract names. */
export function replyData<T>(reply: Record<string, unknown>, tag: string, fields: readonly string[]): T {
  invariant(reply.reply === tag, `the daemon answered ${String(reply.reply)}, not ${tag}`);
  const data = reply.data as Record<string, unknown> | null | undefined;
  invariant(data !== null && typeof data === "object", `the ${tag} reply carries its data`);
  for (const field of fields) invariant(field in data, `the ${tag} reply has ${field}`);
  return data as T;
}
