// The frame of every learning screen: its header and tabs, the settings sheet, and the gate. While
// Noesis is off the frame renders the off state instead of the screen, so no screen reads anything.

import { navigate } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, EmptyState, Skeleton } from "@aether-ui/ui";
import { useCallback, useSyncExternalStore, type ReactNode } from "react";
import { useGate } from "./data.ts";
import { SettingsSheet } from "./SettingsSheet.tsx";
import "./learn.css";

export type LearnView = "sessions" | "session" | "graph" | "me";

export interface OpenGate {
  readonly conn: string;
  readonly status: aether.NoesisStatus;
}

const TABS: readonly { readonly view: LearnView; readonly label: string; readonly href: string }[] = [
  { view: "sessions", label: "Sessions", href: "#/learn" },
  { view: "graph", label: "Knowledge", href: "#/learn/graph" },
  { view: "me", label: "My understanding", href: "#/learn/me" },
];

const SETTINGS_QUERY = "panel=settings";

function subscribeHash(listener: () => void): () => void {
  addEventListener("hashchange", listener);
  return () => removeEventListener("hashchange", listener);
}
const readHash = (): string => location.hash;

/** The settings sheet is open while the hash carries `?panel=settings`, so any screen can link to it. */
export function settingsHref(hash: string = location.hash): string {
  const [path] = hash.split("?", 1);
  return `${path || "#/learn"}?${SETTINGS_QUERY}`;
}

function useSettingsOpen(): [boolean, () => void, () => void] {
  const hash = useSyncExternalStore(subscribeHash, readHash, readHash);
  const [path, query = ""] = hash.split("?", 2);
  const open = query.split("&").includes(SETTINGS_QUERY);
  const show = useCallback(() => navigate(settingsHref(hash)), [hash]);
  const hide = useCallback(() => navigate(path || "#/learn"), [path]);
  return [open, show, hide];
}

export function LearnFrame({ view, title, children }: { view: LearnView; title?: string; children: (gate: OpenGate) => ReactNode }) {
  const gate = useGate();
  const [settingsOpen, showSettings, hideSettings] = useSettingsOpen();
  const conn = gate.state === "disconnected" ? null : gate.conn;
  return (
    <section className="learn" aria-labelledby="learn-title">
      <header className="learn-header">
        <div className="learn-heading">
          <p className="learn-eyebrow">Noesis</p>
          <h1 id="learn-title" className="learn-title">{title ?? "Learn"}</h1>
        </div>
        <nav className="learn-tabs" aria-label="Learning views">
          {TABS.map((tab) => (
            <a key={tab.view} className="learn-tab" href={tab.href} aria-current={tab.view === view || (view === "session" && tab.view === "sessions") ? "page" : undefined}>{tab.label}</a>
          ))}
        </nav>
        {conn !== null && <Button variant="quiet" onClick={showSettings}>Settings</Button>}
      </header>
      <div className="learn-body">{body(gate, showSettings, children)}</div>
      {conn !== null && gate.state !== "unsupported" && <SettingsSheet open={settingsOpen} conn={conn} onClose={hideSettings} />}
    </section>
  );
}

function body(gate: ReturnType<typeof useGate>, showSettings: () => void, children: (gate: OpenGate) => ReactNode): ReactNode {
  switch (gate.state) {
    case "disconnected":
      return <EmptyState title="Not connected to Aether">Learning runs on your Aether daemon. It appears here once the app reconnects.</EmptyState>;
    case "unsupported":
      return <EmptyState title="This daemon has no Noesis">Noesis arrived in Aether 1.3. Update the daemon to learn here.</EmptyState>;
    case "loading":
      return <div className="learn-loading" aria-busy="true" aria-label="Loading Noesis"><Skeleton height="var(--space-12)" /><Skeleton height="var(--space-12)" /></div>;
    case "off":
      return <OffState onSettings={showSettings} />;
    case "on":
      return children({ conn: gate.conn, status: gate.status });
  }
}

function OffState({ onSettings }: { onSettings: () => void }) {
  return (
    <div className="learn-off" data-testid="noesis-off">
      <EmptyState title="Noesis is off" action={<Button variant="primary" onClick={onSettings}>Open Noesis settings</Button>}>
        Noesis teaches through questions: it finds where you stand, plans a path to your goal, and keeps track of what you
        understand. It stays off until the daemon's host turns it on. While it is off, nothing is read or sent.
      </EmptyState>
    </div>
  );
}
