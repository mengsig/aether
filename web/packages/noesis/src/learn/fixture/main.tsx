// The screenshot page: the learning screens in the real shell, connected through core to the faux
// Noesis daemon in the page. Nothing here reaches a real daemon or a model.
// Query: `off` (Noesis disabled), `remote` (not the daemon's host), `refuse` (no usable model),
// `pose` (vote|recall|multi|goal), `conclude` (a finished practice problem), `theme` (light|dark).

import "@aether-ui/noesis";
import { AppProvider, createApp, navigate, selectConnections, useCapabilities, useRoute, useRoutes, useSelector } from "@aether-ui/core";
import { AppShell, ThemeProvider, type ResolvedTheme } from "@aether-ui/ui";
import { StrictMode, useMemo, useSyncExternalStore } from "react";
import { createRoot } from "react-dom/client";
import { invariant } from "../model.ts";
import { FauxNoesis, type HeldQuestion } from "./faux-noesis.ts";
import { GOAL_QUESTION, LESSON, MULTI_QUESTION, PRACTICE, RECALL_QUESTION, scenario, VOTE_QUESTION } from "./scenario.ts";

const params = new URLSearchParams(location.search);
const theme: ResolvedTheme = params.get("theme") === "dark" ? "dark" : "light";
const QUESTIONS: Readonly<Record<string, HeldQuestion>> = { vote: VOTE_QUESTION, recall: RECALL_QUESTION, multi: MULTI_QUESTION, goal: GOAL_QUESTION };

const NO_MODEL = "no ready model for Noesis: set noesis.billing.allow_api to use API models";
const daemon = new FauxNoesis(scenario(!params.has("off"), {
  transport: params.has("remote") ? "ws-aether" : "ipc",
  ...(params.has("refuse") ? { refuse: NO_MODEL } : {}),
}));

/** Plays the teacher's side once the page has connected: a question, or a practice round. */
function play(): void {
  const pose = params.get("pose");
  if (pose !== null) {
    const held = QUESTIONS[pose];
    invariant(held !== undefined, `pose=${pose} names a scenario question`);
    daemon.pose(LESSON.id, held);
  }
  if (params.has("conclude")) {
    daemon.conclude(PRACTICE.id, { problem: "pb_split", outcome: "solved_with_hints", rung: 1, retest: true, miss_class: null,
      solution: "Neither candidate has a majority, so both time out. Randomised timeouts make one of them start the next term first, and it usually wins." });
    daemon.problem(PRACTICE.id, { id: "pb_split_2", mode: "discrimination", posed_at: "", targets: ["c_vote"],
      statement: "A candidate with a shorter log asks you for a vote in a newer term. Which rule decides, and what do you answer?" });
  }
}

const subscribeHash = (listener: () => void) => {
  addEventListener("hashchange", listener);
  return () => removeEventListener("hashchange", listener);
};
const readHash = () => location.hash || "#/learn";

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
if (location.hash === "") location.hash = "#/learn";
createRoot(mount).render(
  <StrictMode>
    <AppProvider app={app}><ThemeProvider initialPreference={theme}><FixtureShell /></ThemeProvider></AppProvider>
  </StrictMode>,
);
setTimeout(play, 400);
