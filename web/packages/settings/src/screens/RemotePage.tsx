import { AETHER_HOST_READS, readInFlight, useApp, useReadPermission, useReread, type ReadEntry } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, ErrorState } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../kit/invariant.ts";
import { Action, RunNote, Section, useRun } from "../kit/kit.tsx";
import { ReadView, recordOf } from "./read.tsx";

const KEY = "settings.remote";
const READ = { cmd: "get_remote" };
const CHANGE = { cmd: "configure_remote" };
const TAGS = ["remote"];

interface Verification { readonly enabled: boolean; readonly after: number }

function remoteStatus(value: unknown): aether.RemoteStatus | null {
  const status = recordOf<aether.RemoteStatus>(value);
  const settings = recordOf<aether.RemoteSettings>(status?.settings);
  if (status === null || settings === null || typeof status.address !== "string") return null;
  if (!Number.isSafeInteger(status.revision) || status.revision < 0) return null;
  if (settings.enabled !== undefined && typeof settings.enabled !== "boolean") return null;
  if (status.listening != null && typeof status.listening !== "string") return null;
  if (status.error != null && typeof status.error !== "string") return null;
  for (const count of [status.devices, status.connections]) {
    if (count !== undefined && (!Number.isSafeInteger(count) || count < 0)) return null;
  }
  return status;
}

function RemoteStatusView({ status }: { readonly status: aether.RemoteStatus }) {
  const enabled = status.settings.enabled === true;
  return (
    <div className="st-stack">
      <div className="st-row"><Badge tone={status.error ? "danger" : enabled ? "success" : "neutral"}>{enabled ? "On" : "Off"}</Badge></div>
      <p className="st-meta">Address: {status.address || "Not configured"}</p>
      <p className="st-meta">{status.listening ? `Listening on ${status.listening}` : "Not listening"}</p>
      {status.devices === undefined ? null : <p className="st-meta">Paired devices: {status.devices}</p>}
      {status.connections === undefined ? null : <p className="st-meta">Connections: {status.connections}</p>}
      {status.error ? <p className="st-note" role="alert" data-state="failed">{status.error}</p> : null}
    </div>
  );
}

function VerificationNote({ expected, entry, retry }: {
  readonly expected: Verification; readonly entry: ReadEntry | undefined; readonly retry: () => void;
}) {
  invariant(expected.after >= 0, "verification starts after a known read order");
  invariant(typeof expected.enabled === "boolean", "verification names the requested state");
  if (!entry || readInFlight(entry) || entry.settled <= expected.after) {
    return <p className="st-note" role="status">Checking remote access…</p>;
  }
  const status = remoteStatus(entry.value);
  if (entry.error !== null || status === null || entry.issued <= expected.after) {
    return <ErrorState title="Could not verify remote access" message={entry.error ?? "The host did not return a current remote status."} onRetry={retry} retryLabel="Retry verification" />;
  }
  if ((status.settings.enabled === true) !== expected.enabled) {
    return <ErrorState title="The host did not apply the change" message={`Remote access is still ${status.settings.enabled ? "on" : "off"}. Check again or retry the change.`} onRetry={retry} retryLabel="Retry verification" />;
  }
  if (status.error) return <ErrorState title="Remote access needs attention" message={status.error} onRetry={retry} retryLabel="Retry verification" />;
  const listening = status.listening ? `, listening on ${status.listening}` : ", not listening";
  return <p className="st-note" role="status">Verified: remote access is {expected.enabled ? `on${listening}` : "off"}</p>;
}

function RemoteAccess({ conn }: { readonly conn: ConnId }) {
  const app = useApp();
  const permission = useReadPermission(conn, READ);
  const entry = useReread(conn, KEY, permission.refusal === null ? READ : null, TAGS);
  const run = useRun(conn);
  const [expected, setExpected] = useState<Verification | null>(null);
  invariant(AETHER_HOST_READS.has(READ.cmd), "the remote status read is passed by the desktop");
  const reread = () => app.reread(conn, KEY, READ, TAGS);
  const turn = async (enabled: boolean) => {
    invariant(permission.refusal === null, "a remote toggle requires the local desktop connection");
    invariant(!readInFlight(entry), "a remote toggle starts from a settled status");
    setExpected(null);
    if (!await run.privileged({ kind: enabled ? "remote.enable" : "remote.disable" }, "Checking remote access…")) return;
    setExpected({ enabled, after: app.store.get().reads[conn]?.[KEY]?.requested ?? 0 });
    await reread();
  };
  if (permission.refusal !== null) {
    return <EmptyState title="Remote access is managed on the host" action={<Badge tone="info">Host only</Badge>}>{permission.refusal}</EmptyState>;
  }
  return (
    <div className="st-stack">
      <Section title="Remote access" hint="When on, paired phones and browsers can reach this daemon over the network while its listener is running.">
        <ReadView label="remote access" entry={entry} onRetry={() => void reread()}>
          {(value) => {
            const status = remoteStatus(value);
            if (status === null) return <ErrorState title="Could not read remote access" message="The daemon returned an invalid remote status." onRetry={() => void reread()} retryLabel="Retry" />;
            const enabled = status.settings.enabled === true;
            return <>
              <RemoteStatusView status={status} />
              {entry?.error ? <Button onClick={() => void reread()}>Retry</Button> : null}
              <Action conn={conn} command={CHANGE} label={enabled ? "Turn off remote access" : "Turn on remote access"}
                variant={enabled ? "danger" : "primary"} busy={run.status.state === "busy" || readInFlight(entry)}
                blocked={entry?.error ? "Refresh the remote status before changing it" : null} onRun={() => turn(!enabled)}
                confirm={{ title: enabled ? "Turn off remote access?" : "Turn on remote access?",
                  body: enabled ? "Paired devices lose access until it is turned on again." : "Paired devices can reach this daemon from its public origins.",
                  confirmLabel: enabled ? "Turn off remote access" : "Turn on remote access" }} />
            </>;
          }}
        </ReadView>
        {expected === null ? <RunNote status={run.status} /> : <VerificationNote expected={expected} entry={entry} retry={() => void reread()} />}
      </Section>
    </div>
  );
}

export function RemotePage({ conn }: { readonly conn: ConnId }) {
  return <RemoteAccess key={conn} conn={conn} />;
}
