// The one Capabilities value (CONTRACTS.md §3, ARCHITECTURE.md §b), computed
// from the host, the connections and the settings that affect the UI.

import type { Capabilities, ConnectionInfo, ConnId, HostAdapter } from "@aether-ui/protocol";
import { invariant, type Json } from "./guards.ts";

export interface CapabilityInputs {
  readonly host: HostAdapter["kind"];
  readonly desktop: boolean;
  readonly managedStart: boolean;
  readonly connections: Readonly<Record<ConnId, ConnectionInfo>>;
  /** `noesis_status` per Aether connection, when read. */
  readonly noesis: Readonly<Record<ConnId, Json | null>>;
}

function noesisFlags(status: Json | null | undefined): { enabled: boolean; engineering: boolean } {
  const effective = (status?.effective ?? {}) as Json;
  const enabled = status?.enabled === true;
  const engineering = effective.engineering_context ?? effective["noesis.engineering_context"];
  return { enabled, engineering: enabled && engineering === true };
}

export function computeCapabilities(inputs: CapabilityInputs): Capabilities {
  invariant(inputs.host === "browser" || inputs.host === "electron", `unknown host ${String(inputs.host)}`);
  invariant(!inputs.desktop || inputs.host === "electron", "only Electron has a desktop bridge");
  const caps: Record<string, boolean | string> = {
    "host.electron": inputs.host === "electron",
    "host.browser": inputs.host === "browser",
    "aether.connected": false,
    "archon.connected": false,
    "noesis.available": false,
    "noesis.engineering_context": false,
    "desktop.notifications": inputs.desktop,
    "desktop.dialogs": inputs.desktop,
    "desktop.deep_links": inputs.desktop,
    "desktop.managed_start": inputs.desktop && inputs.managedStart,
  };
  for (const info of Object.values(inputs.connections)) {
    if (info.state !== "ready") continue;
    caps[`${info.daemon}.connected`] = true;
    for (const feature of info.features) caps[`${info.daemon}.feature.${feature}`] = true;
    if (info.standing) {
      caps[`standing.${info.id}`] = info.standing;
      caps[`can.operate.${info.id}`] = info.standing === "local" || info.standing === "operator";
      caps[`can.host_admin.${info.id}`] = info.standing === "local";
    }
    if (info.daemon === "aether" && info.features.includes("noesis")) {
      const noesis = noesisFlags(inputs.noesis[info.id]);
      if (noesis.enabled) caps["noesis.available"] = true;
      if (noesis.engineering) caps["noesis.engineering_context"] = true;
    }
  }
  return Object.freeze(caps);
}

/** Whether two capability values say the same thing, so an unchanged recompute keeps its identity. */
export function sameCapabilities(a: Capabilities, b: Capabilities): boolean {
  const keys = Object.keys(a);
  return keys.length === Object.keys(b).length && keys.every((key) => a[key] === b[key]);
}
