// Sample routes, connections and rows for the gallery and the shell tests: no daemon needed.

import type { ConnectionInfo, EntityAction, RouteDef } from "@aether-ui/protocol";
import { STATUSES, type Status } from "@aether-ui/ui";
import { DemoScreen, FailingScreen } from "./demo.tsx";

const demo = async () => ({ default: DemoScreen });

export const sampleRoutes: RouteDef[] = [
  { path: "#/fleet", owner: "B1", nav: { label: "Fleet", section: "aether" }, load: demo },
  { path: "#/fleet/topology", owner: "B1", nav: { label: "Topology", section: "aether" }, load: demo },
  { path: "#/inbox", owner: "B3", nav: { label: "Inbox", section: "aether" }, load: demo },
  { path: "#/activity", owner: "B3", nav: { label: "Activity", section: "aether" }, load: demo },
  { path: "#/agents/:conn/:agent", owner: "B2", load: demo },
  { path: "#/archon/queue", owner: "C1", nav: { label: "Queue", section: "archon" }, load: async () => ({ default: FailingScreen }) },
  { path: "#/archon/graph", owner: "C2", nav: { label: "Task graph", section: "archon" }, load: demo },
  { path: "#/archon/history", owner: "C2", nav: { label: "History", section: "archon" }, load: demo },
  { path: "#/learn", owner: "F5", nav: { label: "Learn", section: "learn" }, requires: (caps) => caps["noesis.available"] === true, load: demo },
  { path: "#/settings/*", owner: "B3", nav: { label: "Settings", section: "settings" }, load: demo },
];

export const sampleActions: EntityAction[] = [
  { id: "agent.open", kind: "agent", label: "Open conversation", run: () => undefined },
  { id: "agent.pause", kind: "agent", label: "Pause", requires: (caps, ref) => caps[`can.operate.${ref.conn}`] === true, run: () => undefined },
];

export const sampleConnections: ConnectionInfo[] = [
  { id: "local", daemon: "aether", state: "ready", features: [], label: "studio.local", standing: "local", protocol: 26 },
  { id: "archon", daemon: "archon", state: "backoff", features: [], label: "studio.local/archon" },
];

const RUN_STATES = STATUSES.slice(0, 9) as readonly Status[];
const NAMES = ["planner", "builder", "reviewer", "scout", "scribe", "tester", "archivist", "auditor"];

export function sampleAgent(index: number): { name: string; status: Status; turns: number } {
  const name = `${NAMES[index % NAMES.length] ?? "agent"}-${String(index).padStart(5, "0")}`;
  return { name, status: RUN_STATES[(index * 7) % RUN_STATES.length] ?? "queued", turns: (index * 37) % 400 };
}
