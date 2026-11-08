// Paired devices: listed from core's store and revoked through the desktop's native confirmation
// (host only). Pairing shows a code and a phrase to compare, so it stays with the host's CLI.

import { useApp, usePermission, useReadPermission, useReread } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, Button, EmptyState, Switch } from "@aether-ui/ui";
import { useEffect, useState } from "react";
import { invariant } from "../kit/invariant.ts";
import { Action, RunNote, Section, StandingNotice, useRun, type Run } from "../kit/kit.tsx";
import { formatTime } from "./format.ts";
import { listOf, ReadView, Unexpected } from "./read.tsx";

const KEY = "settings.devices";
const PAIR_SECTION_ID = "settings-pair-device";

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
    <Section title="Pair a new device" id={PAIR_SECTION_ID} hint="Pairing is done on the daemon's host, where the code and the phrase to compare are shown.">
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
  useEffect(() => {
    if (listing.refusal !== null) return;
    const refresh = () => { if (document.visibilityState === "visible") void app.reread(conn, KEY, devicesCommand(includeRevoked), ["devices"]); };
    const timer = setInterval(refresh, 10_000);
    window.addEventListener("focus", refresh);
    document.addEventListener("visibilitychange", refresh);
    return () => { clearInterval(timer); window.removeEventListener("focus", refresh); document.removeEventListener("visibilitychange", refresh); };
  }, [app, conn, includeRevoked, listing.refusal]);

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
          <Switch className="st-switch" label="Show revoked devices" data-testid="settings-show-revoked" checked={showRevoked} onChange={setShowRevoked} />
        ) : null}
        <RunNote status={run.status} />
      </Section>
      <PairSection />
    </div>
  );
}

/** The hash is the route here, so the pairing steps are scrolled to rather than linked by fragment. */
function showPairing(): void {
  const section = document.getElementById(PAIR_SECTION_ID);
  invariant(section !== null, "the pairing steps are on the page");
  section.scrollIntoView({ block: "start" });
}

function DeviceList({ conn, value, run, reread }: { readonly conn: ConnId; readonly value: unknown; readonly run: Run; readonly reread: () => Promise<void> }) {
  const devices = listOf<aether.Device>(value);
  if (devices === null) return <Unexpected label="devices" />;
  if (devices.length === 0) {
    return (
      <EmptyState title="No devices paired" action={<Button size="sm" variant="secondary" onClick={showPairing}>How to pair a device</Button>}>
        Pair a phone or browser to reach this daemon remotely. Pairing starts on the daemon's host, as shown below.
      </EmptyState>
    );
  }
  return (
    <ul className="st-list">
      {devices.map((device) => <DeviceRow key={device.id} conn={conn} device={device} run={run} reread={reread} />)}
    </ul>
  );
}
