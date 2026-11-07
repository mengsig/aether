// "Learn from this" on items, runs, agents and pull requests. Routes are frozen, so the action opens
// F5's `#/learn` with a query its sheet reads; the action is offered only while engineering context is on.

import { navigate } from "@aether-ui/core";
import type { Capabilities, EntityAction, RouteDef } from "@aether-ui/protocol";
import { understandHref, type UnderstandKind } from "./bundle.ts";

const routes: RouteDef[] = [];

/** Core sets this only while Noesis is on and engineering context is effective. */
export const engineeringContextOn = (caps: Capabilities): boolean => caps["noesis.engineering_context"] === true;

function learnFromThis(kind: Exclude<UnderstandKind, "repository">): EntityAction {
  return {
    id: `f6.learn-from-${kind}`,
    kind,
    label: "Learn from this",
    requires: engineeringContextOn,
    run: (ref) => navigate(understandHref({ kind, conn: ref.conn, id: ref.id })),
  };
}

export const actions: EntityAction[] = [learnFromThis("item"), learnFromThis("run"), learnFromThis("agent"), learnFromThis("pull_request")];

export default routes;
