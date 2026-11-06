import { navigate, selectConnections, selectReports, useApp, useCapabilities, useEntityActions, useRoute, useRoutes, useSelector } from "@aether-ui/core";
import type { Report } from "@aether-ui/core";
import type { EntityAction } from "@aether-ui/protocol";
import { HomeScreen } from "@aether-ui/home";
import { AppShell, type ToastMessage } from "@aether-ui/ui";
import { useCallback, useMemo, useState, useSyncExternalStore } from "react";
import { screenTitles } from "./titles.ts";

// Reports stay in the store; the shell shows the newest few the user has not dismissed.
const VISIBLE_NOTICES = 4;

function subscribeHash(listener: () => void): () => void {
  window.addEventListener("hashchange", listener);
  return () => window.removeEventListener("hashchange", listener);
}

const readHash = (): string => window.location.hash || "#/";

const UNEXPLAINED_REPORT = "Something failed without saying why";

/** A report as a toast. Daemons may send a blank message, and a toast must have a title. */
export function reportNotice(report: Report): ToastMessage {
  const title = report.message.trim() === "" ? UNEXPLAINED_REPORT : report.message;
  return { id: report.id, title, tone: "danger", ...(report.code ? { body: report.code } : {}) };
}

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
  const notices = useMemo(() => reports
    .filter((report) => !dismissed.has(report.id))
    .slice(-VISIBLE_NOTICES)
    .map(reportNotice),
  [reports, dismissed]);
  const dismiss = useCallback((id: ToastMessage["id"]) => setDismissed((old) => new Set([...old, id])), []);
  const onActionError = useCallback((action: EntityAction, error: unknown) => {
    app.store.report(`${action.label} failed: ${error instanceof Error ? error.message : String(error)}`);
  }, [app]);

  return (
    <AppShell routes={routes} actions={actions} capabilities={capabilities} match={match} hash={hash}
      onNavigate={navigate} onActionError={onActionError} connections={connections} notices={notices} onDismissNotice={dismiss} titles={screenTitles}
      home={<HomeScreen />} />
  );
}
