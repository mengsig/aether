import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/setup", owner: "B3", load: () => import("./SetupScreen.tsx") },
];

export default routes;
