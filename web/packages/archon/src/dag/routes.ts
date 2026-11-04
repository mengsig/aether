import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/archon/graph", owner: "C2", nav: { label: "Task graph", section: "archon" }, load: () => import("./DagScreen.tsx") },
];

export default routes;
