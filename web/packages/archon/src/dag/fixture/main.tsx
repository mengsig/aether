// The C2 fixture: the task graph, initiative and history screens in the real shell over a faux
// archond at protocol 24. Query: `world` (demo|large), `count` (large items, default 2000), `theme`
// (light|dark). The screenshots and dev/archon-dag-bench.ts serve this directory with Vite.

import "@aether-ui/archon";
import { AppProvider, createApp, navigate, selectConnections, useCapabilities, useRoute, useRoutes, useSelector } from "@aether-ui/core";
import { AppShell, ThemeProvider, type ResolvedTheme } from "@aether-ui/ui";
import { StrictMode, useMemo, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import { invariant } from "../../queue/model.ts";
import { FauxGraphArchond } from "./faux-graph.ts";
import { demoGraphWorld, largeGraphWorld } from "./graph-world.ts";

const params = new URLSearchParams(location.search);
const large = params.get("world") === "large";
const count = Number(params.get("count") ?? 2000);
const theme: ResolvedTheme = params.get("theme") === "dark" ? "dark" : "light";
invariant(Number.isInteger(count) && count >= 1 && count <= 2000, `count=${count} is 1 to 2000`);

const subscribeHash = (listener: () => void) => {
  addEventListener("hashchange", listener);
  return () => removeEventListener("hashchange", listener);
};
const readHash = () => location.hash || "#/archon/graph";

function FixtureShell() {
  const routes = useRoutes();
  const match = useRoute();
  const capabilities = useCapabilities();
  const connectionMap = useSelector(selectConnections);
  const connections = useMemo(() => Object.values(connectionMap), [connectionMap]);
  const hash = useSyncExternalStore(subscribeHash, readHash, readHash);
  return (
    <AppShell routes={routes} actions={[]} capabilities={capabilities} match={match} hash={hash} onNavigate={navigate}
      onActionError={() => undefined} connections={connections} notices={[]} />
  );
}

const faux = new FauxGraphArchond(large ? largeGraphWorld(count) : demoGraphWorld(), { protocol: 24 });
const memory = new Map<string, unknown>();
const app = createApp(
  { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
  { endpoints: [{ daemon: "archon", transport: faux.transport, target: "wss://faux/archon/ws", label: "faux" }] },
);
if (location.hash === "") location.hash = "#/archon/graph";
document.documentElement.dataset.theme = theme;
const mount = document.getElementById("root");
invariant(mount !== null, "the fixture page has a #root");
createRoot(mount).render(
  <StrictMode>
    <AppProvider app={app}><ThemeProvider initialPreference={theme}><FixtureShell /></ThemeProvider></AppProvider>
  </StrictMode>,
);
void app.start();
