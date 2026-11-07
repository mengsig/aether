import type { RouteDef } from "@aether-ui/protocol";
export default [{ path: "#/memory/:conn", owner: "B3", load: () => import("./MemoryScreen.tsx") }] satisfies RouteDef[];
