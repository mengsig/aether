import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/fleet", owner: "B1", nav: { label: "Fleet", section: "aether" }, load: () => import("./FleetScreen.tsx") },
];

export default routes;
