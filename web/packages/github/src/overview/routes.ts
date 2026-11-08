import type { RouteDef } from "@aether-ui/protocol";

export default [{ path: "#/github", owner: "D4", nav: { label: "Overview", section: "github" },
  load: () => import("./GithubOverviewScreen.tsx") }] satisfies RouteDef[];
