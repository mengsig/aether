import type { App } from "@aether-ui/core";
import type { ConnectionInfo } from "@aether-ui/protocol";
import { useEffect, useRef, useState } from "react";
import { loadArchived } from "./archived.ts";
import { invariant } from "./invariant.ts";
import type { FleetSource } from "./rows.ts";

interface Entry {
  info: ConnectionInfo;
  next: string | null;
  ids: Set<string>;
  roots: readonly string[] | undefined;
  started: boolean;
  loading: boolean;
  error: string | undefined;
  generation: number;
}

class ArchiveSession {
  private entries = new Map<string, Entry>();
  private active = true;
  private app: App;
  private changed: () => void;
  constructor(app: App, changed: () => void) {
    this.app = app;
    this.changed = changed;
  }

  updateConnections(infos: readonly ConnectionInfo[]) {
    const ready = new Map(infos.map(info => [info.id, info]));
    invariant(ready.size === infos.length, "archive connections have unique ids");
    invariant(infos.every(info => info.daemon === "aether" && info.state === "ready"), "archive connections are ready Aether daemons");
    for (const [id, entry] of this.entries) {
      const info = ready.get(id);
      if (info && info.features.includes("agents_paging") === entry.info.features.includes("agents_paging")) continue;
      entry.generation += 1;
      this.entries.delete(id);
    }
    for (const info of infos) {
      if (this.entries.has(info.id)) continue;
      const entry: Entry = { info, next: null, ids: new Set(), roots: this.app.store.get().aether[info.id]?.roots, started: false, loading: false, error: undefined, generation: 0 };
      this.entries.set(info.id, entry);
      void this.fetch(entry);
    }
    this.changed();
  }
  stop() { this.active = false; }
  get loading() { return this.entries.size === 0 || [...this.entries.values()].some(entry => entry.loading); }
  get error() { return [...this.entries.values()].find(entry => entry.error)?.error; }
  get hasMore() { return [...this.entries.values()].some(entry => entry.started && entry.next !== null); }
  retry() {
    for (const entry of this.entries.values()) if (entry.error) void this.fetch(entry);
  }
  more() {
    for (const entry of this.entries.values()) if (entry.started && entry.next !== null && !entry.error) void this.fetch(entry);
  }

  reconcile() {
    for (const entry of this.entries.values()) {
      const state = this.app.store.get().aether[entry.info.id];
      const roots = state?.roots;
      const agents = state?.agents ?? {};
      if (roots === entry.roots) {
        for (const id of entry.ids) if (agents[id] === undefined) entry.ids.delete(id);
        continue;
      }
      entry.roots = roots;
      if (![...entry.ids].some(id => agents[id] === undefined)) continue;
      entry.generation += 1;
      entry.ids.clear();
      entry.next = null;
      entry.started = false;
      entry.loading = false;
      entry.error = undefined;
      void this.fetch(entry);
    }
  }

  private async fetch(entry: Entry) {
    if (!this.active || entry.loading) return;
    const generation = ++entry.generation;
    entry.loading = true;
    entry.error = undefined;
    this.changed();
    const current = () => this.active && entry.generation === generation;
    try {
      const page = await loadArchived(this.app, entry.info, entry.started ? entry.next : null, current);
      if (!current() || page === null) return;
      for (const id of page.ids) entry.ids.add(id);
      entry.next = page.next;
      entry.started = true;
    } catch (error) {
      if (!current()) return;
      entry.error = error instanceof Error ? error.message : String(error);
    } finally {
      if (current()) { entry.loading = false; this.changed(); }
    }
  }
}

export function useArchived(app: App, enabled: boolean, token: number, connections: Record<string, ConnectionInfo>, sources: readonly FleetSource[]) {
  const session = useRef<ArchiveSession | null>(null);
  const [status, setStatus] = useState<{ token: number; loading: boolean; error: string | undefined; hasMore: boolean }>({ token: -1, loading: true, error: undefined, hasMore: false });
  useEffect(() => {
    if (!enabled) return;
    const next = new ArchiveSession(app, () => setStatus({ token, loading: next.loading, error: next.error, hasMore: next.hasMore }));
    session.current = next;
    return () => { next.stop(); if (session.current === next) session.current = null; };
  }, [app, enabled, token]);
  useEffect(() => {
    if (!enabled) return;
    session.current?.updateConnections(Object.values(connections).filter(info => info.daemon === "aether" && info.state === "ready"));
  }, [app, enabled, connections]);
  useEffect(() => { if (enabled) session.current?.reconcile(); }, [enabled, sources]);
  return {
    loading: enabled && (status.token !== token || status.loading),
    error: enabled && status.token === token ? status.error : undefined,
    hasMore: enabled && status.token === token && status.hasMore,
    retry: () => session.current?.retry(),
    more: () => session.current?.more(),
  };
}
