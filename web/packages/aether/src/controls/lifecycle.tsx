// Lifecycle controls for one agent: interrupt, pause or resume (optionally with its delegates),
// wake, compact, restart, archive or restore, and cancel. Restart, archive and cancel ask first.

import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { useDraft } from "@aether-ui/core";
import { Switch } from "@aether-ui/ui";
import { invariant } from "../fleet/invariant.ts";
import { Action, RunNote, Section, useRun } from "./kit.tsx";

export function terminalReason(agent: aether.Agent): string | null {
  const state = agent.status.state;
  if (state !== "completed" && state !== "failed" && state !== "cancelled" && state !== "imported") return null;
  const next = agent.restarted_as ? ` Continue with ${agent.restarted_as}.`
    : state === "failed" ? " Wake it to continue." : state === "cancelled" ? " Restart it to create a successor."
      : state === "completed" ? " Completed agents cannot be continued here." : "";
  return `This agent is ${state}.${next}`;
}

export function LifecyclePanel({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  invariant(agent.id !== "", "lifecycle controls name their agent");
  const run = useRun(conn);
  const draft = useDraft(`controls/${agent.id}/lifecycle`, { subtree: false });
  const { subtree } = draft.value;
  const id = agent.id;
  const busy = run.status.state === "busy";
  const ended = terminalReason(agent);
  const recoveryBlocked = agent.restarted_as ? `Already restarted as ${agent.restarted_as}.` : null;
  const wakeBlocked = agent.status.state === "failed" ? recoveryBlocked : "Only failed agents can be woken.";
  const restartBlocked = agent.status.state === "failed" || agent.status.state === "cancelled" ? recoveryBlocked : "Only failed or cancelled agents can be restarted.";
  const paused = agent.paused === true;
  const resumeBlocked = paused && ended ? `${ended} Select “Include the agents it delegated to” to resume its subtree.` : ended;
  const archived = agent.archived === true;
  const canResumeWait = agent.status.state === "waiting" || (agent.status.state === "blocked" && (agent.status.reason.block === "inspection" || agent.status.reason.block === "exhausted"));
  const hold = paused ? aether.resume({ agent: id, ...(subtree ? { subtree } : {}) }) : aether.pause({ agent: id, ...(subtree ? { subtree } : {}) });
  const shelf = aether.archive({ target: { agent: id }, archived: !archived });
  return (
    <>
      <Section title="Steady it" hint="Stop the current turn, or hold the agent until you resume it.">
        <div className="b3-row">
          <Action conn={conn} command={aether.interrupt({ agent: id })} label="Interrupt" busy={busy} blocked={ended} onRun={() => run.send(aether.interrupt({ agent: id }), "Interrupted")} />
          <Action conn={conn} command={hold} label={paused ? "Resume" : "Pause"} busy={busy} blocked={paused && subtree ? null : resumeBlocked} onRun={() => run.send(hold, paused ? "Resumed" : "Paused")} />
          {!paused && canResumeWait ? <Action conn={conn} command={aether.resume({ agent: id })} label="Resume" busy={busy} onRun={() => run.send(aether.resume({ agent: id }), "Resumed")} /> : null}
          <Switch label="Include the agents it delegated to" checked={subtree} onChange={subtree => draft.update({ subtree })} />
        </div>
      </Section>
      <Section title="Nudge it" hint="Wake a failed agent to continue in place, or compact a live agent’s context to free room.">
        <div className="b3-row">
          <Action conn={conn} command={aether.wake({ agent: id })} label="Wake" busy={busy} blocked={wakeBlocked} onRun={() => run.send(aether.wake({ agent: id }), "Woken")} />
          <Action conn={conn} command={aether.compact({ agent: id })} label="Compact context" busy={busy} blocked={ended} onRun={() => run.send(aether.compact({ agent: id }), "Compacting")} />
        </div>
      </Section>
      <Section title="End or replace it" hint="Restart, archive or cancel work; restore an archived agent to the fleet.">
        <div className="b3-row">
          <Action conn={conn} command={aether.restart({ agent: id })} label="Restart" variant={restartBlocked === null ? "primary" : "secondary"} busy={busy} blocked={restartBlocked}
            confirm={{ title: `Restart ${agent.name}?`, body: "A new agent continues from the last checkpoint and recent transcript; this agent remains as it ended.", confirmLabel: "Restart" }}
            onRun={() => run.send(aether.restart({ agent: id }), "Restarted")} />
          <Action conn={conn} command={shelf} label={archived ? "Restore" : "Archive"} busy={busy} blocked={!archived && ended === null ? "Only finished agents can be archived." : null}
            confirm={archived ? undefined : { title: `Archive ${agent.name}?`, body: "It leaves the fleet view. You can restore it later.", confirmLabel: "Archive" }}
            onRun={() => run.send(shelf, archived ? "Restored" : "Archived")} />
          <Action conn={conn} command={aether.cancel({ agent: id })} label="Cancel agent" variant="danger" busy={busy}
            confirm={{ title: `Cancel ${agent.name}?`, body: ended === null ? "This agent and its live delegated agents stop, and their open work is abandoned. This cannot be undone." : "Any live delegated agents stop, and their open work is abandoned. This cannot be undone.", confirmLabel: "Cancel agent", reason: "optional" }}
            onRun={(reason) => run.send(aether.cancel({ agent: id, ...(reason ? { reason } : {}) }), "Cancelled")} />
        </div>
      </Section>
      <RunNote status={run.status} />
    </>
  );
}
