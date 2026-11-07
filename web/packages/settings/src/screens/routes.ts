import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/settings/*", owner: "B3", nav: { label: "Settings", section: "settings" }, load: () => import("./SettingsScreen.tsx") },
];

export default routes;
