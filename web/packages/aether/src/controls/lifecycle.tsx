// Lifecycle controls for one agent: interrupt, pause or resume (optionally with its delegates),
// wake, compact, restart, archive or restore, and cancel. Restart, archive and cancel ask first.

import type { ConnId } from "@aether-ui/protocol";
import { aether } from "@aether-ui/protocol";
import { useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { Action, RunNote, Section, useRun } from "./kit.tsx";

export function LifecyclePanel({ conn, agent }: { readonly conn: ConnId; readonly agent: aether.Agent }) {
  invariant(agent.id !== "", "lifecycle controls name their agent");
  const run = useRun(conn);
  const [subtree, setSubtree] = useState(false);
  const id = agent.id;
  const busy = run.status.state === "busy";
  const paused = agent.paused === true;
  const archived = agent.archived === true;
  const hold = paused ? aether.resume({ agent: id, ...(subtree ? { subtree } : {}) }) : aether.pause({ agent: id, ...(subtree ? { subtree } : {}) });
  const shelf = aether.archive({ target: { agent: id }, archived: !archived });
  return (
    <>
      <Section title="Steady it" hint="Stop the current turn, or hold the agent until you resume it.">
        <div className="b3-row">
          <Action conn={conn} command={aether.interrupt({ agent: id })} label="Interrupt" busy={busy} onRun={() => run.send(aether.interrupt({ agent: id }), "Interrupted")} />
          <Action conn={conn} command={hold} label={paused ? "Resume" : "Pause"} busy={busy} onRun={() => run.send(hold, paused ? "Resumed" : "Paused")} />
          <label className="b3-check">
            <input type="checkbox" checked={subtree} onChange={(event) => setSubtree(event.target.checked)} />
            <span>Include the agents it delegated to</span>
          </label>
        </div>
      </Section>
      <Section title="Nudge it" hint="Wake a waiting agent, or compact its context to free room.">
        <div className="b3-row">
          <Action conn={conn} command={aether.wake({ agent: id })} label="Wake" busy={busy} onRun={() => run.send(aether.wake({ agent: id }), "Woken")} />
          <Action conn={conn} command={aether.compact({ agent: id })} label="Compact context" busy={busy} onRun={() => run.send(aether.compact({ agent: id }), "Compacting")} />
        </div>
      </Section>
      <Section title="End or replace it" hint="These change the agent for good, so each asks first.">
        <div className="b3-row">
          <Action conn={conn} command={aether.restart({ agent: id })} label="Restart" busy={busy}
            confirm={{ title: `Restart ${agent.name}?`, body: "A fresh agent replaces this one with the same configuration; the current turn is lost.", confirmLabel: "Restart" }}
            onRun={() => run.send(aether.restart({ agent: id }), "Restarted")} />
          <Action conn={conn} command={shelf} label={archived ? "Restore" : "Archive"} busy={busy}
            confirm={archived ? undefined : { title: `Archive ${agent.name}?`, body: "It leaves the fleet view. You can restore it later.", confirmLabel: "Archive" }}
            onRun={() => run.send(shelf, archived ? "Restored" : "Archived")} />
          <Action conn={conn} command={aether.cancel({ agent: id })} label="Cancel agent" variant="danger" busy={busy}
            confirm={{ title: `Cancel ${agent.name}?`, body: "The agent stops for good and its open work is abandoned. This cannot be undone.", confirmLabel: "Cancel agent", reason: "optional" }}
            onRun={(reason) => run.send(aether.cancel({ agent: id, ...(reason ? { reason } : {}) }), "Cancelled")} />
        </div>
      </Section>
      <RunNote status={run.status} />
    </>
  );
}
