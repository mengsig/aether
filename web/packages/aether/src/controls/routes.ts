import { navigate, routeHref } from "@aether-ui/core";
import type { EntityAction, RouteDef } from "@aether-ui/protocol";

// A detail route: reached from an agent's entity action, so it has no nav entry.
const routes: RouteDef[] = [
  { path: "#/controls/:conn/:agent", owner: "B3", load: () => import("./ControlsScreen.tsx") },
];

export const actions: EntityAction[] = [
  {
    id: "b3.agent-controls",
    kind: "agent",
    label: "Open controls",
    run: (ref) => navigate(routeHref("#/controls/:conn/:agent", { conn: ref.conn, agent: ref.id })),
  },
];

export default routes;
