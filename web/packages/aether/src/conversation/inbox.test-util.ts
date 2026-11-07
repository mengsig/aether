// Tests only: an agent's inbox records (`get_inbox`) as the chat reads them, built from short helpers.

import type { aether } from "@aether-ui/protocol";
import { withDeliveries, type Deliveries } from "./inbox.ts";

export const INBOX_AGENT = "agt_01JINBOX00000000000000001";

export const operator = (text: string): aether.InboxKind =>
  ({ kind: "message", message_id: "msg_1", from: { actor: "user", id: "local_socket#7" }, content: [{ type: "text", text }] } as aether.InboxKind);
export const peer = (text: string): aether.InboxKind =>
  ({ kind: "message", message_id: "msg_2", from: { actor: "agent", id: "agt_peer" }, content: [{ type: "text", text }] } as aether.InboxKind);
export const done = (task: string, summary: string): aether.InboxKind =>
  ({ kind: "task_completed", task, result: { summary, artifacts: [] } } as unknown as aether.InboxKind);
export const cancelled = (task: string): aether.InboxKind => ({ kind: "task_cancelled", task, reason: null } as aether.InboxKind);
export const childFailed = (agent: string): aether.InboxKind => ({ kind: "child_failed", agent, error: "boom" } as unknown as aether.InboxKind);
export const approved = (approval: string): aether.InboxKind =>
  ({ kind: "approval_resolved", approval, decision: { decision: "approved" } } as unknown as aether.InboxKind);
export const wake: aether.InboxKind = { kind: "wake", reason: "timer" } as aether.InboxKind;

/** The records an agent's inbox read returns, by seq, read after every turn up to `covers` was written. */
export function records(kinds: Readonly<Record<number, aether.InboxKind>>, covers = Number.MAX_SAFE_INTEGER): Deliveries {
  const items = Object.entries(kinds).map(([seq, kind]): aether.InboxItem =>
    ({ id: `ibx_${seq}`, agent_id: INBOX_AGENT, seq: Number(seq), delivered_at: "2026-11-07T09:00:00Z", kind }));
  return withDeliveries(null, { items, after: 0, covers }, INBOX_AGENT);
}
