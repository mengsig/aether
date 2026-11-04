// The dev-only gallery (gallery.html): the shell with sample routes in either theme,
// for visual review, screenshots and in-browser axe runs. Never part of the build.

import { AppShell, isThemePreference, ThemeProvider } from "@aether-ui/ui";
import type { RouteDef } from "@aether-ui/protocol";
import { StrictMode, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import "../app.css";
import "./gallery.css";
import { sampleActions, sampleConnections, sampleRoutes } from "./fixtures.tsx";

const capabilities = { "can.operate.local": true };

function subscribeHash(listener: () => void): () => void {
  window.addEventListener("hashchange", listener);
  return () => window.removeEventListener("hashchange", listener);
}

function match(hash: string): { route: RouteDef; params: Record<string, string> } | null {
  const path = hash.split("?")[0] ?? "";
  const route = sampleRoutes.find((candidate) => candidate.path === path || (candidate.path.endsWith("/*") && path.startsWith(candidate.path.slice(0, -1))));
  return route ? { route, params: {} } : null;
}

function Gallery() {
  const hash = useSyncExternalStore(subscribeHash, () => window.location.hash || "#/fleet", () => "#/fleet");
  return (
    <AppShell routes={sampleRoutes} actions={sampleActions} capabilities={capabilities} match={match(hash)} hash={hash}
      onNavigate={(href) => { window.location.hash = href; }} onActionError={(action, error) => { throw new Error(`${action.id} failed`, { cause: error }); }}
      connections={sampleConnections} notices={[{ id: 1, title: "Reconnecting to Archon", body: "Retrying in 4 s", tone: "warning" }]} />
  );
}

const theme = new URLSearchParams(window.location.search).get("theme");
const mount = document.getElementById("root");
if (mount === null) throw new Error("gallery.html has no #root element");
createRoot(mount).render(
  <StrictMode>
    <ThemeProvider initialPreference={isThemePreference(theme) ? theme : "system"}>
      <Gallery />
    </ThemeProvider>
  </StrictMode>,
);
