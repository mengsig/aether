// Paired devices: listed from core's store and revoked through the desktop's native confirmation
// (host only). Pairing shows a code and a phrase to compare, so it stays with the host's CLI.

import { useApp, usePermission, useReadPermission, useReread } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, EmptyState } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../kit/invariant.ts";
import { Action, RunNote, Section, StandingNotice, useRun, type Run } from "../kit/kit.tsx";
import { formatTime } from "./format.ts";
import { listOf, ReadView, Unexpected } from "./read.tsx";

const KEY = "settings.devices";

export const devicesCommand = (includeRevoked: boolean) => (includeRevoked ? { cmd: "list_devices", include_revoked: true } : { cmd: "list_devices" });

function DeviceRow({ conn, device, run, reread }: { readonly conn: ConnId; readonly device: aether.Device; readonly run: Run; readonly reread: () => Promise<void> }) {
  invariant(device.id !== "" && device.name !== "", "a device has an id and a name");
  const revoke = async () => {
    if (await run.privileged({ kind: "device.revoke", device: device.id }, `${device.name} was revoked`)) await reread();
  };
  return (
    <li className="st-item" aria-label={device.name}>
      <div className="st-item-head">
        <h4 className="st-item-title">{device.name}</h4>
        <span className="st-row">
          <Badge tone="accent">{device.role}</Badge>
          {device.current ? <Badge tone="info">This device</Badge> : null}
          {device.push ? <Badge tone="success">Push on</Badge> : null}
          {device.revoked_at ? <Badge tone="danger">Revoked</Badge> : null}
        </span>
      </div>
      <p className="st-meta">Added {formatTime(device.created_at)} · Last seen {formatTime(device.last_seen_at)}</p>
      {device.revoked_at ? null : (
        <Action conn={conn} command={{ cmd: "revoke_device", id: device.id }} label="Revoke" variant="danger" onRun={revoke}
          confirm={{ title: `Revoke ${device.name}?`, body: "This device is signed out at once and can only come back by pairing again.", confirmLabel: "Revoke device" }} />
      )}
    </li>
  );
}

/** Pairing needs the code shown and the claimant's phrase compared on the host; the desktop has no such flow yet. */
function PairSection() {
  return (
    <Section title="Pair a new device" hint="Pairing is done on the daemon's host, where the code and the phrase to compare are shown.">
      <p className="st-prose">Run this on the host, scan the code with the new device, then compare the phrase on both screens before confirming:</p>
      <p className="st-code">aether remote pair --role operator</p>
      <p className="st-meta">Use <span className="st-code">--role observer</span> for a device that may only watch.</p>
    </Section>
  );
}

export function DevicesPage({ conn }: { readonly conn: ConnId }) {
  const app = useApp();
  const [showRevoked, setShowRevoked] = useState(false);
  const listing = usePermission(conn, { cmd: "list_devices" });
  const revokedListing = useReadPermission(conn, devicesCommand(true));
  const includeRevoked = showRevoked && revokedListing.refusal === null;
  const command = listing.refusal === null ? devicesCommand(includeRevoked) : null;
  const entry = useReread(conn, KEY, command, ["devices"]);
  const run = useRun(conn);
  const reread = () => app.reread(conn, KEY, devicesCommand(includeRevoked), ["devices"]);

  return (
    <div className="st-stack">
      <StandingNotice conn={conn} />
      <Section title="Devices" hint="Phones and browsers paired with this daemon.">
        {command === null ? (
          <EmptyState title="Devices are not available here">{listing.refusal}</EmptyState>
        ) : (
          <ReadView label="devices" entry={entry} onRetry={() => void reread()}>
            {(value) => <DeviceList conn={conn} value={value} run={run} reread={reread} />}
          </ReadView>
        )}
        {revokedListing.refusal === null ? (
          <label className="st-check">
            <input type="checkbox" checked={showRevoked} onChange={(event) => setShowRevoked(event.target.checked)} />
            Show revoked devices
          </label>
        ) : null}
        <RunNote status={run.status} />
      </Section>
      <PairSection />
    </div>
  );
}

function DeviceList({ conn, value, run, reread }: { readonly conn: ConnId; readonly value: unknown; readonly run: Run; readonly reread: () => Promise<void> }) {
  const devices = listOf<aether.Device>(value);
  if (devices === null) return <Unexpected label="devices" />;
  if (devices.length === 0) return <EmptyState title="No devices paired">Pair a phone or browser to reach this daemon remotely.</EmptyState>;
  return (
    <ul className="st-list">
      {devices.map((device) => <DeviceRow key={device.id} conn={conn} device={device} run={run} reread={reread} />)}
    </ul>
  );
}
