import type { RouteDef } from "@aether-ui/protocol";
export default [{ path: "#/activity", owner: "B3", nav: { label: "Activity", section: "aether" }, load: () => import("./ActivityScreen.tsx") }] satisfies RouteDef[];
