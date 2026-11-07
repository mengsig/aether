import { navigate, selectConnections, selectReports, useApp, useCapabilities, useEntityActions, useRoute, useRoutes, useSelector } from "@aether-ui/core";
import type { EntityAction } from "@aether-ui/protocol";
import { HomeScreen } from "@aether-ui/home";
import { ArchonToolsNotice, Welcome } from "@aether-ui/settings";
import { AppShell, type ToastMessage } from "@aether-ui/ui";
import { useCallback, useMemo, useState, useSyncExternalStore } from "react";
import { useNoticeLifetime } from "./notice-lifetime.ts";
import { noticesOf } from "./notices.ts";
import { entityCommands, routeOffered, selectPaletteSources, sameSources } from "./palette-entities.ts";
import { screenTitles } from "./titles.ts";

function subscribeHash(listener: () => void): () => void {
  window.addEventListener("hashchange", listener);
  return () => window.removeEventListener("hashchange", listener);
}

const readHash = (): string => window.location.hash || "#/";

/** Archon's screens say so, with the fix, when Archon's tools are not connected to Aether. */
export function bannerFor(hash: string) {
  return hash.startsWith("#/archon/") || hash === "#/archon" ? <ArchonToolsNotice /> : null;
}

export interface ShellRootProps {
  /** The palette's recent destinations to start from, and where each change is saved. */
  readonly initialRecents: readonly string[];
  readonly onRecentsChange: (recents: readonly string[]) => void;
}

/** Feeds the shell everything it renders from core: routes, actions, capabilities, the route, connections, reports and palette entities. */
export function ShellRoot({ initialRecents, onRecentsChange }: ShellRootProps) {
  const app = useApp();
  const routes = useRoutes();
  const actions = useEntityActions();
  const capabilities = useCapabilities();
  const match = useRoute();
  const hash = useSyncExternalStore(subscribeHash, readHash, () => "#/");
  const connectionMap = useSelector(selectConnections);
  const reports = useSelector(selectReports);
  const sources = useSelector(selectPaletteSources, sameSources);
  const [dismissed, setDismissed] = useState<ReadonlySet<ToastMessage["id"]>>(() => new Set());

  const connections = useMemo(() => Object.values(connectionMap), [connectionMap]);
  const notices = useMemo(() => noticesOf(reports, dismissed), [reports, dismissed]);
  const paletteEntities = useMemo(
    () => entityCommands(sources, { navigate, offered: (pattern) => routeOffered(routes, capabilities, pattern) }),
    [sources, routes, capabilities],
  );
  const dismiss = useCallback((id: ToastMessage["id"]) => setDismissed((old) => new Set([...old, id])), []);
  const dismissAll = useCallback(() => setDismissed((old) => new Set([...old, ...notices.map((notice) => notice.id)])), [notices]);
  useNoticeLifetime(notices, dismiss);
  const onActionError = useCallback((action: EntityAction, error: unknown) => {
    app.store.report(`${action.label} failed: ${error instanceof Error ? error.message : String(error)}`);
  }, [app]);

  return (
    <AppShell routes={routes} actions={actions} capabilities={capabilities} match={match} hash={hash}
      onNavigate={navigate} onActionError={onActionError} connections={connections} notices={notices} onDismissNotice={dismiss} onDismissAllNotices={dismissAll} titles={screenTitles}
      home={<Welcome connected={<HomeScreen />} />} banner={bannerFor(hash)} commands={paletteEntities} initialRecents={initialRecents} onRecentsChange={onRecentsChange} />
  );
}
