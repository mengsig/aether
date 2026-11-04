// The remote listener, changed only on the daemon's own host through the desktop's native
// confirmation. Its status (get_remote) is a host-only read the desktop bridge does not pass yet.

import { usePermission, useReadPermission } from "@aether-ui/core";
import type { ConnId } from "@aether-ui/protocol";
import { Badge, EmptyState } from "@aether-ui/ui";
import { Action, RunNote, Section, useRun } from "../kit/kit.tsx";

const CHANGE = { cmd: "configure_remote" };

export function RemotePage({ conn }: { readonly conn: ConnId }) {
  const change = usePermission(conn, CHANGE);
  const status = useReadPermission(conn, { cmd: "get_remote" });
  const run = useRun(conn);
  if (change.refusal !== null) {
    return (
      <div className="st-stack">
        <EmptyState title="Remote access is managed on the host" action={<Badge tone="info">Host only</Badge>}>{change.refusal}</EmptyState>
      </div>
    );
  }
  const turn = (kind: "remote.enable" | "remote.disable", done: string) => () => run.privileged({ kind }, done);
  return (
    <div className="st-stack">
      <Section title="Remote access" hint="Lets paired phones and browsers reach this daemon over the network.">
        {status.refusal === null ? null : <p className="st-meta" role="note">{status.refusal}, so its current state is not shown here.</p>}
        <div className="st-row">
          <Action conn={conn} command={CHANGE} label="Turn on" variant="primary" onRun={turn("remote.enable", "Remote access turned on")}
            confirm={{ title: "Turn on remote access?", body: "Paired devices can reach this daemon from its public origins.", confirmLabel: "Turn on remote access" }} />
          <Action conn={conn} command={CHANGE} label="Turn off" variant="danger" onRun={turn("remote.disable", "Remote access turned off")}
            confirm={{ title: "Turn off remote access?", body: "Paired devices lose access until it is turned on again.", confirmLabel: "Turn off remote access" }} />
        </div>
        <RunNote status={run.status} />
      </Section>
    </div>
  );
}
