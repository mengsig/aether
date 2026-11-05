import type { EntityAction, RouteDef } from "@aether-ui/protocol";
import { ACTIONS } from "./actions.ts";

// A detail route: reached from links and entity actions, so it has no nav entry.
const routes: RouteDef[] = [
  { path: "#/github/:host/:owner/:repo/:number", owner: "D4", load: () => import("./GithubScreen.tsx") },
];

export const actions: EntityAction[] = [...ACTIONS];

export default routes;
