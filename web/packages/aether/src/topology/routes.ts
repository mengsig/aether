import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/fleet/topology", owner: "B1", nav: { label: "Topology", section: "aether" }, load: () => import("./TopologyScreen.tsx") },
];

export default routes;
