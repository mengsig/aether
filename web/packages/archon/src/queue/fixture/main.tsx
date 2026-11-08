// The screenshot fixture: the queue, item and pipeline screens in the real shell over a faux archond.
// Query: `protocol` (24|23, default 24), `role` (operator|observer), `theme` (light|dark).
// Serve this directory with Vite and the package aliases; nothing here reaches a daemon.

import "@aether-ui/archon";
import { AppProvider, createApp, navigate, selectConnections, useCapabilities, useRoute, useRoutes, useSelector } from "@aether-ui/core";
import { AppShell, ThemeProvider, type ResolvedTheme } from "@aether-ui/ui";
import { StrictMode, useMemo, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import { invariant } from "../model.ts";
import { FauxArchond } from "./faux-archond.ts";
import { demoWorld } from "./world.ts";

const params = new URLSearchParams(location.search);
const protocol = Number(params.get("protocol") ?? 24);
const role = params.get("role") === "observer" ? "observer" : "operator";
const theme: ResolvedTheme = params.get("theme") === "dark" ? "dark" : "light";
invariant(protocol === 23 || protocol === 24, `protocol=${protocol} is 23 or 24`);

const subscribeHash = (listener: () => void) => {
  addEventListener("hashchange", listener);
  return () => removeEventListener("hashchange", listener);
};
const readHash = () => location.hash || "#/archon/queue";

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

const world = demoWorld();
if (params.get("gate") === "long") {
  const entry = world.items.get("qi_pairing");
  invariant(entry !== undefined, "the fixture has a proposal gate");
  const gates = entry.detail.gates as Record<string, unknown>[];
  invariant(gates.length === 1, "the fixture has one proposal gate");
  entry.detail = { ...entry.detail, gates: gates.map((gate) => ({ ...gate, summary: `Review the proposal at /workspace/${"long-path".repeat(60)} before deciding.` })) };
}
const faux = new FauxArchond(world, { protocol, role });
const memory = new Map<string, unknown>();
const app = createApp(
  { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
  { endpoints: [{ daemon: "archon", transport: faux.transport, target: "wss://faux/archon/ws", label: "faux" }] },
);
if (location.hash === "") location.hash = "#/archon/queue";
document.documentElement.dataset.theme = theme;
const mount = document.getElementById("root");
invariant(mount !== null, "the fixture page has a #root");
createRoot(mount).render(
  <StrictMode>
    <AppProvider app={app}><ThemeProvider initialPreference={theme}><FixtureShell /></ThemeProvider></AppProvider>
  </StrictMode>,
);
void app.start();
