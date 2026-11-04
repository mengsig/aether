import type { RouteDef } from "@aether-ui/protocol";

const routes: RouteDef[] = [
  { path: "#/inbox", owner: "B3", nav: { label: "Inbox", section: "aether" }, load: () => import("./InboxScreen.tsx") },
];

export default routes;
