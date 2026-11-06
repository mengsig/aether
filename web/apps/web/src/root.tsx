import { navigate, selectConnections, selectReports, useApp, useCapabilities, useEntityActions, useRoute, useRoutes, useSelector } from "@aether-ui/core";
import type { EntityAction } from "@aether-ui/protocol";
import { HomeScreen } from "@aether-ui/home";
import { AppShell, type ToastMessage } from "@aether-ui/ui";
import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { NOTICE_MS, noticesOf } from "./notices.ts";
import { screenTitles } from "./titles.ts";

function subscribeHash(listener: () => void): () => void {
  window.addEventListener("hashchange", listener);
  return () => window.removeEventListener("hashchange", listener);
}

const readHash = (): string => window.location.hash || "#/";

/** Feeds the shell everything it renders from core: routes, actions, capabilities, the route, connections and reports. */
export function ShellRoot() {
  const app = useApp();
  const routes = useRoutes();
  const actions = useEntityActions();
  const capabilities = useCapabilities();
  const match = useRoute();
  const hash = useSyncExternalStore(subscribeHash, readHash, () => "#/");
  const connectionMap = useSelector(selectConnections);
  const reports = useSelector(selectReports);
  const [dismissed, setDismissed] = useState<ReadonlySet<ToastMessage["id"]>>(() => new Set());

  const connections = useMemo(() => Object.values(connectionMap), [connectionMap]);
  const notices = useMemo(() => noticesOf(reports, dismissed), [reports, dismissed]);
  const dismiss = useCallback((id: ToastMessage["id"]) => setDismissed((old) => new Set([...old, id])), []);
  const dismissAll = useCallback(() => setDismissed((old) => new Set([...old, ...notices.map((notice) => notice.id)])), [notices]);
  useNoticeLifetime(notices, dismiss);
  const onActionError = useCallback((action: EntityAction, error: unknown) => {
    app.store.report(`${action.label} failed: ${error instanceof Error ? error.message : String(error)}`);
  }, [app]);

  return (
    <AppShell routes={routes} actions={actions} capabilities={capabilities} match={match} hash={hash}
      onNavigate={navigate} onActionError={onActionError} connections={connections} notices={notices} onDismissNotice={dismiss} onDismissAllNotices={dismissAll} titles={screenTitles}
      home={<HomeScreen />} />
  );
}

/** Each toast leaves on its own after NOTICE_MS; a repeat of the same failure renews it under a new id. */
function useNoticeLifetime(notices: readonly ToastMessage[], dismiss: (id: ToastMessage["id"]) => void): void {
  const ids = notices.map((notice) => notice.id).join(",");
  useEffect(() => {
    if (ids === "") return;
    const timers = ids.split(",").map((id) => setTimeout(() => dismiss(Number(id)), NOTICE_MS));
    return () => { for (const timer of timers) clearTimeout(timer); };
  }, [ids, dismiss]);
}
