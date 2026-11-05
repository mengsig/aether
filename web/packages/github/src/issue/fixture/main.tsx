// The D4 fixture: the GitHub pages in the real shell over a faux archond, and with `aether=1` a faux
// aetherd too. Query: `aether` (0|1), `role` (operator|observer), `theme` (light|dark). Screenshots serve it with Vite.

import "@aether-ui/github";
import { AppProvider, createApp, navigate, selectConnections, useCapabilities, useEntityActions, useRoute, useRoutes, useSelector, type Endpoint } from "@aether-ui/core";
import { AppShell, ThemeProvider, type ResolvedTheme } from "@aether-ui/ui";
import { StrictMode, useMemo, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import { invariant } from "../ref.ts";
import { FauxAether, FauxArchon } from "./faux.ts";
import { demoWorld } from "./world.ts";

const params = new URLSearchParams(location.search);
const theme: ResolvedTheme = params.get("theme") === "dark" ? "dark" : "light";
const role = params.get("role") === "observer" ? "observer" : "operator";

const subscribeHash = (listener: () => void) => {
  addEventListener("hashchange", listener);
  return () => removeEventListener("hashchange", listener);
};
const readHash = () => location.hash || "#/github/github.com/acme/site/41";

function FixtureShell() {
  const routes = useRoutes();
  const actions = useEntityActions();
  const match = useRoute();
  const capabilities = useCapabilities();
  const connectionMap = useSelector(selectConnections);
  const connections = useMemo(() => Object.values(connectionMap), [connectionMap]);
  const hash = useSyncExternalStore(subscribeHash, readHash, readHash);
  return (
    <AppShell routes={routes} actions={actions} capabilities={capabilities} match={match} hash={hash} onNavigate={navigate}
      onActionError={() => undefined} connections={connections} notices={[]} />
  );
}

const world = demoWorld();
const endpoints: Endpoint[] = [{ daemon: "archon", transport: new FauxArchon(world, { role }).transport, target: "wss://faux/archon/ws", label: "faux" }];
if (params.get("aether") === "1") endpoints.push({ daemon: "aether", transport: new FauxAether(world.agents).transport, target: "faux-aetherd", label: "studio" });
const app = createApp({ kind: "browser", storage: { get: async () => undefined, set: async () => {} } }, { endpoints });
if (location.hash === "") location.hash = "#/github/github.com/acme/site/41";
document.documentElement.dataset.theme = theme;
const mount = document.getElementById("root");
invariant(mount !== null, "the fixture page has a #root");
createRoot(mount).render(
  <StrictMode>
    <AppProvider app={app}><ThemeProvider initialPreference={theme}><FixtureShell /></ThemeProvider></AppProvider>
  </StrictMode>,
);
void app.start();
