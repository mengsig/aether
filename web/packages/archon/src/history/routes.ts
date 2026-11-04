import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/archon/history", owner: "C2", nav: { label: "History", section: "archon" }, load: () => import("./HistoryScreen.tsx") },
];

export default routes;
