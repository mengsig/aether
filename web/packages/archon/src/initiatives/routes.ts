import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/archon/initiatives/:id", owner: "C2", load: () => import("./InitiativeScreen.tsx") },
];

export default routes;
