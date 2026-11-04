import type { RouteDef } from "@aether-ui/protocol";

// A detail route: reached from the fleet, topology and links, so it has no nav entry.
const routes: RouteDef[] = [
  { path: "#/agents/:conn/:agent", owner: "B2", load: () => import("./AgentScreen.tsx") },
];

export default routes;
