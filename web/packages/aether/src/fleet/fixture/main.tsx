// The perf and screenshot fixture: the fleet and topology screens in the real shell over a
// synthetic fleet. Query: `agents` (default 5000), `theme` (light|dark), `churn` (status changes per 100 ms).
// Serve this directory with Vite and the package aliases; nothing here reaches a daemon.

import "@aether-ui/aether";
import { AppProvider, createApp, navigate, selectConnections, useCapabilities, useRoute, useRoutes, useSelector, type Store } from "@aether-ui/core";
import { AppShell, ThemeProvider, type ResolvedTheme } from "@aether-ui/ui";
import { StrictMode, useMemo, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import { invariant } from "../invariant.ts";
import { fixtureAgents, fixtureConnection, FIXTURE_CONN, seedFleet } from "./agents.ts";

const params = new URLSearchParams(location.search);
const agentCount = Number(params.get("agents") ?? 5_000);
const theme: ResolvedTheme = params.get("theme") === "dark" ? "dark" : "light";
const churn = Number(params.get("churn") ?? 0);
invariant(Number.isInteger(agentCount) && agentCount >= 0, `agents=${agentCount} is a whole number`);
invariant(Number.isInteger(churn) && churn >= 0, `churn=${churn} is a whole number`);

const STATES = ["running", "completed", "ready", "created"] as const;

/** Changes `perTick` agents' states every 100 ms, as a busy daemon's events would. */
function startChurn(store: Store, perTick: number): void {
  let tick = 0;
  setInterval(() => {
    tick += 1;
    store.updateAether(FIXTURE_CONN, (state) => {
      const agents = { ...state.agents };
      for (let n = 0; n < perTick; n += 1) {
        const id = `agt_${String((tick * 7919 + n * 104_729) % agentCount).padStart(5, "0")}`;
        const agent = agents[id];
        if (agent !== undefined) agents[id] = { ...agent, status: { state: STATES[(tick + n) % STATES.length] ?? "running" } };
      }
      return { ...state, agents };
    });
  }, 100);
}

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

const memory = new Map<string, unknown>();
const app = createApp({ kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } });
seedFleet(app.store, fixtureConnection(), fixtureAgents(agentCount));
if (churn > 0) startChurn(app.store, churn);
if (location.hash === "") location.hash = "#/fleet";
document.documentElement.dataset.theme = theme;
const mount = document.getElementById("root");
invariant(mount !== null, "the fixture page has a #root");
createRoot(mount).render(
  <StrictMode>
    <AppProvider app={app}><ThemeProvider initialPreference={theme}><FixtureShell /></ThemeProvider></AppProvider>
  </StrictMode>,
);
