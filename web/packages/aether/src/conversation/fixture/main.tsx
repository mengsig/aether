// The screenshot and timing page: the agent screen in the real shell, connected through core to a
// faux daemon in the page. Query: `entries` (default 20000), `theme` (light|dark), `stream` (0 to stay idle).
// Serve this directory with Vite and the package aliases; nothing here reaches a real daemon.

import "@aether-ui/aether";
import { AppProvider, createApp, navigate, selectConnections, useCapabilities, useRoute, useRoutes, useSelector, type App } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { AppShell, ThemeProvider, type ResolvedTheme } from "@aether-ui/ui";
import { StrictMode, useMemo, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import { invariant } from "../../fleet/invariant.ts";
import { FauxDaemon } from "./faux-daemon.ts";
import { FAUX_AGENT, FAUX_PARENT, fauxAgent, fauxConversation } from "./transcript.ts";

const params = new URLSearchParams(location.search);
const entryCount = Number(params.get("entries") ?? 20_000);
const theme: ResolvedTheme = params.get("theme") === "dark" ? "dark" : "light";
const streaming = params.get("stream") !== "0";
invariant(Number.isInteger(entryCount) && entryCount >= 0, `entries=${entryCount} is a whole number`);

const INSTANCE = "faux-demo";
const CONN = `aether:${INSTANCE}`;
const INV = "inv_zz_live";
const timing: Record<string, number> = {};
(globalThis as { fixtureTiming?: Record<string, number> }).fixtureTiming = timing;

const daemon = new FauxDaemon({ instance: INSTANCE, agents: [fauxAgent(FAUX_PARENT), fauxAgent()] });
daemon.seed(FAUX_AGENT, fauxConversation(entryCount));

const REASONING = ["The reviewer asked for the reconnect case. ", "A late delta must not grow the tail again, ", "so the committed message has to replace it."];
const ANSWER = ["Both cases now hold. ", "The **tombstone** keeps the tail empty after the commit, ", "and the screen hides any tail whose invocation is already in the transcript.\n\n", "Next: run `make ci` and "];

/** Streams reasoning, then text, then a tool call's input, and leaves the message uncommitted. */
function stream(): void {
  const deltas: aether.Delta[] = [
    ...REASONING.map((text): aether.Delta => ({ delta: "thinking", text })),
    ...ANSWER.map((text): aether.Delta => ({ delta: "text", text })),
    { delta: "tool_input", name: "bash", partial_json: "", tool_use_id: "toolu_zz_live" },
    { delta: "tool_input", name: "bash", partial_json: "{\"command\": \"make ci\"", tool_use_id: "toolu_zz_live" },
  ];
  deltas.forEach((delta, index) => setTimeout(() => daemon.delta(FAUX_AGENT, INV, index + 1, delta), 150 * (index + 1)));
}

/** Records when the whole transcript is in the store and its newest row is drawn. */
function measure(app: App, started: number): void {
  const check = () => {
    const loaded = app.store.getSnapshot().aether[CONN]?.transcripts[FAUX_AGENT]?.entries.length === entryCount;
    const rows = document.querySelectorAll(".conv-row");
    const last = [...rows].some((row) => row.getAttribute("aria-posinset") === row.getAttribute("aria-setsize"));
    if (!loaded || !last) return void requestAnimationFrame(check);
    timing.openMs = performance.now() - started;
    timing.domRows = rows.length;
    if (streaming) stream();
  };
  requestAnimationFrame(check);
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
const app = createApp(
  { kind: "browser", storage: { get: async (key) => memory.get(key), set: async (key, value) => { memory.set(key, value); } } },
  { endpoints: [{ daemon: "aether", transport: daemon.transport, target: "faux://aetherd", label: "faux-aetherd" }] },
);
document.documentElement.dataset.theme = theme;
const mount = document.getElementById("root");
invariant(mount !== null, "the fixture page has a #root");
await app.start();
location.hash = `#/agents/${encodeURIComponent(CONN)}/${FAUX_AGENT}`;
measure(app, performance.now());
createRoot(mount).render(
  <StrictMode>
    <AppProvider app={app}><ThemeProvider initialPreference={theme}><FixtureShell /></ThemeProvider></AppProvider>
  </StrictMode>,
);
