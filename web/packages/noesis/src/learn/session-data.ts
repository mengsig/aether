// What a lesson reads beyond its session: every agent's transcript and the current agent's live
// stream. Only the current agent is watched for deltas (core keeps three); the others are reread once
// and grow from the durable events after that.

import { selectStreams, useApp, useSelector, type State } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { useCallback, useEffect, useMemo } from "react";
import { errorText } from "./data.ts";
import { invariant } from "./model.ts";
import { liveItems, sessionAgents, type AgentTranscript, type TimelineItem } from "./timeline.ts";

const NO_ENTRIES: readonly aether.TranscriptEntry[] = [];

/** Follows the current agent's deltas and reads the earlier agents' transcripts. */
function useFollowAgents(conn: string, current: string, earlier: readonly string[]): void {
  const app = useApp();
  const others = earlier.join(" ");
  useEffect(() => {
    const watch = (agents: readonly string[]) => app.manager.watchAgents(conn, agents).catch((error: unknown) => {
      app.store.report(`Could not follow the session's conversation: ${errorText(error)}`, conn);
    });
    void watch([current]);
    return () => void watch([]);
  }, [app, conn, current]);
  useEffect(() => {
    for (const agent of others === "" ? [] : others.split(" ")) {
      app.manager.readTranscript(conn, agent).catch((error: unknown) => {
        app.store.report(`Could not read the session's earlier conversation: ${errorText(error)}`, conn);
      });
    }
  }, [app, conn, others]);
}

/** Every agent's transcript, oldest agent first, as the store holds it. */
export function useLessonTranscripts(conn: string, session: aether.NoesisSession): readonly AgentTranscript[] {
  const agents = useMemo(() => sessionAgents(session), [session]);
  invariant(agents.some((entry) => entry.agent === session.agent), "the current agent is one of the session's");
  useFollowAgents(conn, session.agent, useMemo(() => agents.filter((entry) => entry.agent !== session.agent).map((entry) => entry.agent), [agents, session.agent]));
  const held = useSelector(useCallback((state: State) => state.aether[conn]?.transcripts, [conn]));
  return useMemo(() => agents.map((agent) => ({ agent, entries: held?.[agent.agent]?.entries ?? NO_ENTRIES })), [agents, held]);
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
