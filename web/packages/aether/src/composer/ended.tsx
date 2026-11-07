// Why an ended agent takes no more messages, and the step that goes on from there.

import { routeHref } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import { Action, RunNote, useRun } from "../controls/kit.tsx";

/** One line on why `agent` takes no messages, or null while it still does. */
export function endedReason(agent: aether.Agent): string | null {
  const status = agent.status;
  switch (status.state) {
    case "completed": return `${agent.name} finished its task${status.summary ? `: ${status.summary}` : "."}`;
    case "failed": return `${agent.name} failed: ${status.error}`;
    case "cancelled": return `${agent.name} was cancelled${status.reason ? `: ${status.reason}` : "."}`;
    case "imported": return `${agent.name} is a record imported from another daemon.`;
    default: return null;
  }
}

export function EndedNotice({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  const reason = endedReason(agent);
  invariant(reason !== null, "only an ended agent explains why it takes no messages");
  invariant(agent.id !== "", "the notice names its agent");
  const run = useRun(conn);
  const failed = agent.status.state === "failed";
  return (
    <div className="cmp-ended" role="note" aria-label="This chat has ended">
      <p className="cmp-ended-title">This chat has ended</p>
      <p className="cmp-ended-body">{reason} An ended agent takes no more messages.</p>
      <div className="cmp-ended-actions">
        {failed ? (
          <Action conn={conn} command={aether.wake({ agent: agent.id })} label="Wake it to continue" variant="primary" busy={run.status.state === "busy"}
            onRun={() => run.send(aether.wake({ agent: agent.id }), "Woken")} />
        ) : null}
        <a className="ui-link" href="#/activity">Start a new chat</a>
        <a className="ui-link" href={routeHref("#/controls/:conn/:agent", { conn, agent: agent.id })}>Open its controls</a>
      </div>
      <RunNote status={run.status} />
    </div>
  );
}
