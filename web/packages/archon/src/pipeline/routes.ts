import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/archon/items/:item/runs/:run", owner: "C1", load: () => import("./PipelineScreen.tsx") },
];

export default routes;
