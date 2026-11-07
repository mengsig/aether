// `#/settings/*`: the settings pages as tabs. The daemon the pages act on is the first ready Aether
// connection unless the person picks another; that pick is view state, the connections are core's.

import { navigate, selectConnections, useSelector } from "@aether-ui/core";
import type { ConnectionInfo, ConnId, RouteProps } from "@aether-ui/protocol";
import { EmptyState, Tabs, type TabItem } from "@aether-ui/ui";
import { useMemo, useState, type ReactNode } from "react";
import { invariant } from "../kit/invariant.ts";
import { SelectField } from "../kit/select.tsx";
import { ConnectionsPage } from "./ConnectionsPage.tsx";
import { DevicesPage } from "./DevicesPage.tsx";
import { NotificationsPage } from "./NotificationsPage.tsx";
import { ProvidersPage } from "./ProvidersPage.tsx";
import { RemotePage } from "./RemotePage.tsx";
import "./settings.css";

export const PAGES = ["connections", "devices", "remote", "providers", "notifications"] as const;
type Page = (typeof PAGES)[number];

const LABELS: Readonly<Record<Page, string>> = {
  connections: "Connections", devices: "Devices", remote: "Remote", providers: "Providers", notifications: "Notifications",
};

/** The page a route names; the first one for `#/settings` and for a name that is not a page. */
export function pageOf(rest: string | undefined): Page {
  const first = (rest ?? "").split("/")[0];
  return PAGES.find((page) => page === first) ?? "connections";
}

/** The chosen connection if it still exists, else the first ready one, else the first. */
export function pickConnection(candidates: readonly ConnectionInfo[], chosen: ConnId | null): ConnectionInfo | undefined {
  invariant(candidates.every((c) => c.daemon === "aether"), "only Aether connections are offered");
  return candidates.find((c) => c.id === chosen) ?? candidates.find((c) => c.state === "ready") ?? candidates[0];
}

function ConnectionPicker({ candidates, value, onPick }: { readonly candidates: readonly ConnectionInfo[]; readonly value: ConnId; readonly onPick: (id: ConnId) => void }) {
  if (candidates.length < 2) return null;
  return (
    <SelectField className="st-picker" label="Daemon" value={value} onChange={onPick} options={candidates.map((c) => ({ value: c.id, label: c.label }))} />
  );
}

function pagePanel(page: Page, conn: ConnId | null): ReactNode {
  if (page === "connections") return <ConnectionsPage />;
  if (conn === null) return <EmptyState title="Not connected to Aether" action={<a className="ui-link" href="#/">Set up or pair a computer</a>}>Connect to Aether to change its settings.</EmptyState>;
  switch (page) {
    case "devices": return <DevicesPage key={conn} conn={conn} />;
    case "remote": return <RemotePage key={conn} conn={conn} />;
    case "providers": return <ProvidersPage key={conn} conn={conn} />;
    case "notifications": return <NotificationsPage key={conn} conn={conn} />;
  }
}

export default function SettingsScreen({ params }: RouteProps) {
  const page = pageOf(params["*"]);
  const connections = useSelector(selectConnections);
  const [chosen, setChosen] = useState<ConnId | null>(null);
  const candidates = useMemo(() => Object.values(connections).filter((c) => c.daemon === "aether"), [connections]);
  const conn = pickConnection(candidates, chosen)?.id ?? null;
  const tabs: TabItem[] = PAGES.map((id) => ({ id, label: LABELS[id], panel: pagePanel(id, conn) }));
  return (
    <div className="ui-page st-screen">
      <header className="ui-page-head st-screen-head">
        <h2 className="ui-visually-hidden st-screen-title">Settings</h2>
        <p className="ui-page-lede st-screen-sub">Connections, devices, remote access, providers and notifications.</p>
      </header>
      {conn !== null && page !== "connections" ? <ConnectionPicker candidates={candidates} value={conn} onPick={setChosen} /> : null}
      <Tabs label="Settings pages" tabs={tabs} selected={page} onSelect={(id) => navigate(`#/settings/${id}`)} />
    </div>
  );
}
