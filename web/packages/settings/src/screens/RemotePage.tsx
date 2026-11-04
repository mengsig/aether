// The remote listener: read only on the daemon's own host (get_remote is host only), changed
// through the desktop's native confirmation.

import { useApp, usePermission, useReread } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, EmptyState } from "@aether-ui/ui";
import { invariant } from "../kit/invariant.ts";
import { Action, RunNote, Section, StandingNotice, useRun } from "../kit/kit.tsx";
import { plural } from "./format.ts";
import { ReadView, recordOf, Unexpected } from "./read.tsx";

const KEY = "settings.remote";
const COMMAND = { cmd: "get_remote" };

function RemoteFacts({ status }: { readonly status: aether.RemoteStatus }) {
  const origins = status.settings.public_origins ?? [];
  const mounts = status.settings.mounts ?? [];
  return (
    <>
      <dl className="st-facts">
        <div><dt>Address</dt><dd className="st-code">{status.address}</dd></div>
        <div><dt>Listening</dt><dd className="st-code">{status.listening ?? "Not listening"}</dd></div>
        <div><dt>Connections</dt><dd>{status.connections ?? 0}</dd></div>
        <div><dt>Paired devices</dt><dd>{status.devices ?? 0}</dd></div>
      </dl>
      {status.error ? <p className="st-note" role="alert" data-state="failed">{status.error}</p> : null}
      <div className="st-stack">
        <h4 className="st-item-title">Public origins</h4>
        {origins.length === 0 ? <p className="st-meta">None configured.</p> : (
          <ul className="st-list">{origins.map((origin) => <li key={origin} className="st-item st-code">{origin}</li>)}</ul>
        )}
        <h4 className="st-item-title">Mounts</h4>
        {mounts.length === 0 ? <p className="st-meta">No daemons are mounted.</p> : (
          <ul className="st-list">
            {mounts.map((mount) => <li key={mount.prefix} className="st-item st-code">{mount.prefix} → {mount.target} ({mount.kind})</li>)}
          </ul>
        )}
      </div>
    </>
  );
}

export function RemotePage({ conn }: { readonly conn: ConnId }) {
  const app = useApp();
  const { refusal } = usePermission(conn, COMMAND);
  const entry = useReread(conn, KEY, refusal === null ? COMMAND : null, ["remote"]);
  const run = useRun(conn);
  const reread = () => app.reread(conn, KEY, COMMAND, ["remote"]);
  if (refusal !== null) {
    return (
      <div className="st-stack">
        <EmptyState title="Remote access is managed on the host" action={<Badge tone="info">Host only</Badge>}>{refusal}</EmptyState>
      </div>
    );
  }
  return (
    <div className="st-stack">
      <StandingNotice conn={conn} />
      <Section title="Remote access" hint="Lets paired phones and browsers reach this daemon over the network.">
        <ReadView label="remote access" entry={entry} onRetry={() => void reread()}>
          {(value) => <RemoteBody conn={conn} value={value} run={run} reread={reread} />}
        </ReadView>
        <RunNote status={run.status} />
      </Section>
    </div>
  );
}

function RemoteBody({ conn, value, run, reread }: { readonly conn: ConnId; readonly value: unknown; readonly run: ReturnType<typeof useRun>; readonly reread: () => Promise<void> }) {
  const status = recordOf<aether.RemoteStatus>(value);
  if (status === null || recordOf(status.settings) === null) return <Unexpected label="remote access" />;
  const enabled = status.settings.enabled === true;
  invariant(typeof status.address === "string", "the remote status names its address");
  const change = async () => {
    const kind = enabled ? "remote.disable" : "remote.enable";
    if (await run.privileged({ kind }, enabled ? "Remote access turned off" : "Remote access turned on")) await reread();
  };
  const confirm = enabled
    ? { title: "Turn off remote access?", body: `Paired devices lose access until it is turned on again. ${plural(status.connections ?? 0, "connection")} close now.`, confirmLabel: "Turn off remote access" }
    : { title: "Turn on remote access?", body: "Paired devices can reach this daemon from the public origins below.", confirmLabel: "Turn on remote access" };
  return (
    <>
      <div className="st-row">
        <Badge tone={enabled ? "success" : "neutral"}>{enabled ? "Enabled" : "Disabled"}</Badge>
        <Action conn={conn} command={{ cmd: "configure_remote" }} label={enabled ? "Turn off" : "Turn on"} variant={enabled ? "danger" : "primary"} confirm={confirm} onRun={change} />
      </div>
      <RemoteFacts status={status} />
    </>
  );
}
