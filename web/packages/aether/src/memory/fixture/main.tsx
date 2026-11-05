// The screenshot page: the memory screen in the real shell, connected through core to the scripted
// daemon with the Memory 2.0 demo data. Query: `theme` (light|dark), `role` (local|operator|observer),
// `v2` (0 for a daemon without memory_v2). Serve with Vite and the package aliases; no real daemon.

import "@aether-ui/aether";
import { AppProvider, createApp, navigate, selectConnections, useCapabilities, useRoute, useRoutes, useSelector } from "@aether-ui/core";
import { AppShell, ThemeProvider, type ResolvedTheme } from "@aether-ui/ui";
import { StrictMode, useMemo, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import { invariant } from "../../fleet/invariant.ts";
import { ScriptedDaemon, type Role } from "../../controls/fixture/daemon.ts";
import { demoAgents } from "../../controls/fixture/demo.ts";
import { memoryScript, V2_FEATURES } from "./data.ts";

const params = new URLSearchParams(location.search);
const theme: ResolvedTheme = params.get("theme") === "dark" ? "dark" : "light";
const role = (params.get("role") ?? "local") as Role;
invariant(["local", "operator", "observer"].includes(role), `role=${role} is one the scripted daemon plays`);
const features = params.get("v2") === "0" ? V2_FEATURES.filter((feature) => feature !== "memory_v2") : V2_FEATURES;

const subscribeHash = (listener: () => void) => {
  addEventListener("hashchange", listener);
  return () => removeEventListener("hashchange", listener);
};
const readHash = () => location.hash || "#/fleet";

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

const daemon = new ScriptedDaemon({ instance: "studio", role, features, agents: demoAgents(), script: memoryScript() });
const storage = new Map<string, unknown>();
const app = createApp(
  { kind: "browser", storage: { get: async (key) => storage.get(key), set: async (key, value) => { storage.set(key, value); } } },
  { endpoints: [{ daemon: "aether", transport: daemon.transport, target: "faux-aetherd", label: "studio" }] },
);
document.documentElement.dataset.theme = theme;
const mount = document.getElementById("root");
invariant(mount !== null, "the fixture page has a #root");
await app.start();
location.hash = "#/memory/aether:studio";
createRoot(mount).render(
  <StrictMode>
    <AppProvider app={app}><ThemeProvider initialPreference={theme}><FixtureShell /></ThemeProvider></AppProvider>
  </StrictMode>,
);
