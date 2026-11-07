import type { RouteDef } from "@aether-ui/protocol";
export default [
  { path: "#/activity", owner: "B3", nav: { label: "Activity", section: "aether" }, load: () => import("./ActivityScreen.tsx") },
  { path: "#/activity/start", owner: "B3", load: () => import("./StartScreen.tsx") },
] satisfies RouteDef[];
