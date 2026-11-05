// The bundle source for an Aether agent: core follows the agent while the sheet is open, as the
// agent page does, so its record and transcript land in the store.

import { selectAgent, selectTranscript, useApp, useSelector } from "@aether-ui/core";
import { useEffect, useMemo } from "react";
import { errorText } from "../learn/data.ts";
import { invariant } from "../learn/model.ts";
import { agentBundle } from "./agent-bundle.ts";
import { notConnected, useConnectionReady, useCoreRead, type BundleRead, type Reader } from "./read.ts";

const NO_ENTRIES: never[] = [];

const watchAgent: Reader = (app, conn, agents) => app.manager.watchAgents(conn, agents);

/** Stops following the agent when the sheet closes; a failure is reported, as the agent page does. */
function useUnwatchOnClose(conn: string, ready: boolean): void {
  const app = useApp();
  useEffect(() => {
    if (!ready) return undefined;
    return () => {
      app.manager.watchAgents(conn, []).catch((error: unknown) => app.store.report(`Could not stop following the agent: ${errorText(error)}`, conn));
    };
  }, [app, conn, ready]);
}

export function useAgentSource(conn: string, id: string): BundleRead {
  invariant(id.length > 0, "an agent source names its agent");
  const ready = useConnectionReady(conn, "aether");
  const agent = useSelector(useMemo(() => selectAgent(conn, id), [conn, id]));
  const transcript = useSelector(useMemo(() => selectTranscript(conn, id), [conn, id]));
  const watch = useCoreRead(conn, ready, [id], watchAgent);
  useUnwatchOnClose(conn, ready);
  const entries = transcript?.entries ?? NO_ENTRIES;
  const bundle = useMemo(() => (agent === undefined ? null : agentBundle(agent, entries)), [agent, entries]);
  if (bundle !== null) return transcript === undefined && !watch.settled ? { state: "loading" } : { state: "ready", bundle };
  if (!ready) return notConnected("Aether", conn);
  if (watch.error !== null) return { state: "error", title: "Could not read the agent", message: watch.error, retry: watch.retry };
  return watch.settled ? { state: "missing", title: "No such agent", message: `${conn} has no agent ${id}.` } : { state: "loading" };
}
