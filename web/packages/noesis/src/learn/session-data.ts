// What a lesson reads beyond its session: every agent's transcript and the current agent's live
// stream. Only the current agent is watched for deltas (core keeps three); the others are reread once
// and grow from the durable events after that.

import { selectStreams, useApp, useSelector, type State } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { useCallback, useEffect, useMemo, useState } from "react";
import { errorText } from "./data.ts";
import { invariant } from "./model.ts";
import { liveItems, sessionAgents, type AgentTranscript, type TimelineItem } from "./timeline.ts";

const NO_ENTRIES: readonly aether.TranscriptEntry[] = [];

/** Follows the current agent's deltas and reads the earlier agents' transcripts. */
function useFollowAgents(conn: string, current: string, earlier: readonly string[]): boolean {
  const app = useApp();
  const others = earlier.join(" ");
  const [resumed, setResumed] = useState<{ agent: string; alive: boolean } | null>(null);
  useEffect(() => {
    let live = true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const report = (error: unknown) => {
      app.store.report(`Could not follow the session's conversation: ${errorText(error)}`, conn);
    };
    const check = async () => {
      if (!live) return;
      try {
        const reply = await app.manager.send(conn, { cmd: "get_agent", id: current });
        if (!live) return;
        const agent = reply.data as aether.Agent | undefined;
        invariant(reply.reply === "agent" && agent?.id === current, "the liveness read names the watched agent");
        setResumed({ agent: current, alive: agent.engine_alive === true });
        if (agent.engine_alive === true) timer = setTimeout(() => void check(), 1_000);
      } catch (error) {
        if (live) report(error);
      }
    };
    void app.manager.watchAgents(conn, [current]).then(check, report);
    return () => {
      live = false;
      clearTimeout(timer);
      void app.manager.watchAgents(conn, []).catch(report);
    };
  }, [app, conn, current]);
  useEffect(() => {
    for (const agent of others === "" ? [] : others.split(" ")) {
      app.manager.readTranscript(conn, agent).catch((error: unknown) => {
        app.store.report(`Could not read the session's earlier conversation: ${errorText(error)}`, conn);
      });
    }
  }, [app, conn, others]);
  return resumed?.agent === current && resumed.alive;
}

/** Every agent's transcript, oldest agent first, as the store holds it. */
export function useLessonTranscripts(conn: string, session: aether.NoesisSession): { transcripts: readonly AgentTranscript[]; resumed: boolean } {
  const agents = useMemo(() => sessionAgents(session), [session]);
  invariant(agents.some((entry) => entry.agent === session.agent), "the current agent is one of the session's");
  const resumed = useFollowAgents(conn, session.agent, useMemo(() => agents.filter((entry) => entry.agent !== session.agent).map((entry) => entry.agent), [agents, session.agent]));
  const held = useSelector(useCallback((state: State) => state.aether[conn]?.transcripts, [conn]));
  const transcripts = useMemo(() => agents.map((agent) => ({ agent, entries: held?.[agent.agent]?.entries ?? NO_ENTRIES })), [agents, held]);
  return { transcripts, resumed };
}

/** The current agent's text while it streams, before its message commits. */
export function useLiveText(conn: string, agent: string): readonly TimelineItem[] {
  const streams = useSelector(useMemo(() => selectStreams(conn), [conn]));
  return useMemo(() => liveItems(streams, agent), [streams, agent]);
}

/** A turn start without visible text still indicates that the teacher is writing. */
export function useTeacherWriting(conn: string, agent: string): boolean {
  const streams = useSelector(useMemo(() => selectStreams(conn), [conn]));
  return Object.values(streams).some((stream) => stream.agent === agent && !stream.committed && !stream.recovering);
}
