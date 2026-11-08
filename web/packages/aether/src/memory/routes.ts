import type { RouteDef } from "@aether-ui/protocol";
export default [
  { path: "#/memory", owner: "B3", nav: { label: "Memory", section: "aether" }, load: () => import("./MemoryConnections.tsx") },
  { path: "#/memory/:conn", owner: "B3", load: () => import("./MemoryScreen.tsx") },
] satisfies RouteDef[];
