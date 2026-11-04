import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/archon/items/:item", owner: "C1", load: () => import("./ItemScreen.tsx") },
];

export default routes;
