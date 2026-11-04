import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/archon/queue", owner: "C1", nav: { label: "Queue", section: "archon" }, load: () => import("./QueueScreen.tsx") },
];

export default routes;
