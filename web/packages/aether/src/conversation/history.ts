import { useApp, useCommand, useSelector } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { useEffect, useMemo, useState } from "react";
import { invariant } from "../fleet/invariant.ts";

const TYPES = ["agent.started", "agent.turn_completed", "agent.interrupted", "agent.usage_updated", "invocation.completed", "invocation.failed"];
const PAGE_SIZE = 1000;
type Send = ReturnType<typeof useCommand>;

async function readHistory(send: Send, agent: string): Promise<aether.Event[]> {
  const events: aether.Event[] = [];
  let after = 0;
  for (;;) {
    const reply = await send({ cmd: "get_events", after, limit: PAGE_SIZE, query: { agent, types: TYPES } });
    invariant(reply.reply === "events" && Array.isArray(reply.data), "history answers with an event list");
    const page = reply.data as aether.Event[];
    for (const event of page) {
      invariant(Number.isSafeInteger(event.seq) && event.seq > after, "history advances in sequence order");
      after = event.seq;
      if (event.agent_id === agent) events.push(event);
    }
    if (page.length < PAGE_SIZE) return events;
  }
}

export function useConversationHistory(conn: ConnId, agent: string, live: readonly aether.Event[]): { events: readonly aether.Event[]; loaded: boolean } {
  const app = useApp();
  const send = useCommand(conn);
  const ready = useSelector(state => state.connections[conn]?.state === "ready");
  const [history, setHistory] = useState<{ conn: ConnId; agent: string; events: aether.Event[] } | null>(null);
  useEffect(() => {
    if (!ready) return undefined;
    let active = true;
    void readHistory(send, agent).then(events => {
      if (active) setHistory({ conn, agent, events });
    }).catch((error: unknown) => {
      if (active) app.store.report(`Could not read conversation usage: ${error instanceof Error ? error.message : String(error)}`, conn);
    });
    return () => { active = false; };
  }, [agent, app, conn, ready, send]);
  const loaded = history?.agent === agent && history.conn === conn;
  const events = useMemo(() => {
    const bySeq = new Map<number, aether.Event>();
    for (const event of [...(loaded ? history.events : []), ...live]) if (event.agent_id === agent) bySeq.set(event.seq, event);
    return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
  }, [agent, history, live, loaded]);
  return { events, loaded };
}
