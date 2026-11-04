// Paired devices: listed from core's store, revoked and paired through the desktop's native
// confirmation (host only). Whatever a pairing returns is never shown, as it may be a credential.

import { normalizeDeviceName, useApp, usePermission, useReadPermission, useReread } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Badge, EmptyState, Field } from "@aether-ui/ui";
import { useState } from "react";
import { invariant } from "../kit/invariant.ts";
import { SelectField } from "../kit/select.tsx";
import { Action, RunNote, Section, StandingNotice, useRun, type Run } from "../kit/kit.tsx";
import { formatTime } from "./format.ts";
import { listOf, ReadView, Unexpected } from "./read.tsx";

const KEY = "settings.devices";
const ROLES = [
  { value: "observer", label: "Observer: can watch, not change" },
  { value: "operator", label: "Operator: can send and approve" },
];
const PAIRED = "Pairing started; finish it in the desktop dialog";

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

/** The device name as it will be sent, or why it cannot be (an expected input error, shown beside the button). */
function checkName(name: string): { readonly name: string; readonly problem: string | null } {
  if (name.trim() === "") return { name: "", problem: "Name the device first" };
  try {
    return { name: normalizeDeviceName(name), problem: null };
  } catch (error) {
    return { name: "", problem: error instanceof Error ? error.message : String(error) };
  }
}

function PairSection({ conn, run }: { readonly conn: ConnId; readonly run: Run }) {
  const [name, setName] = useState("");
  const [role, setRole] = useState<aether.DeviceRole>("observer");
  const checked = checkName(name);
  const clean = checked.name;
  const pair = async () => {
    invariant(clean !== "", "a pairing names its device");
    if (await run.privileged({ kind: "device.pair", name: clean, role }, PAIRED)) setName("");
  };
  return (
    <Section title="Pair a new device" hint="Starts pairing on the host. The code and phrase appear only in the desktop dialog.">
      <Field label="Device name" value={name} maxLength={64} onChange={(event) => setName(event.target.value)} placeholder="Kitchen tablet" />
      <SelectField label="Role" value={role} onChange={(next) => setRole(next as aether.DeviceRole)} options={ROLES} />
      <Action conn={conn} command={{ cmd: "start_pairing" }} label="Pair a new device" variant="primary" blocked={checked.problem} onRun={pair}
        confirm={{ title: `Pair ${clean || "a new device"} as ${role}?`, body: "The desktop app asks you to confirm before pairing starts.", confirmLabel: "Start pairing" }} />
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
      <PairSection conn={conn} run={run} />
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
