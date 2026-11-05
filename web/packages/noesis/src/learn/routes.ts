import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/learn", owner: "F5", nav: { label: "Sessions", section: "learn" }, load: () => import("./SessionsScreen.tsx") },
  { path: "#/learn/sessions/:id", owner: "F5", load: () => import("./SessionScreen.tsx") },
  { path: "#/learn/graph", owner: "F5", nav: { label: "Knowledge", section: "learn" }, load: () => import("./GraphScreen.tsx") },
  { path: "#/learn/me", owner: "F5", nav: { label: "My understanding", section: "learn" }, load: () => import("./UnderstandingScreen.tsx") },
];

export default routes;
