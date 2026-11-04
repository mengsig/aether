// The route and entity-action registries (CONTRACTS.md §2.5). Feature
// packages register at import time; the shell reads them as props.

import type { EntityAction, EntityKind, RouteDef } from "@aether-ui/protocol";
import { invariant, string } from "./guards.ts";

const ENTITY_KINDS: ReadonlySet<EntityKind> = new Set(["agent", "run", "item", "stage", "initiative", "issue", "pull_request", "proposition"]);
const ROUTE_PATH = /^#\/[a-z0-9:_*/-]*$/;

export class Registry {
  private routeList: readonly RouteDef[] = [];
  private actionList: readonly EntityAction[] = [];
  private readonly listeners = new Set<() => void>();

  registerRoutes(defs: readonly RouteDef[]): void {
    const taken = new Set(this.routeList.map((route) => route.path));
    for (const def of defs) {
      string(def.owner, "route owner");
      if (!ROUTE_PATH.test(def.path)) throw new TypeError(`Route path ${def.path} must look like #/segment/:param`);
      if (taken.has(def.path)) throw new Error(`Route ${def.path} is already registered`);
      taken.add(def.path);
    }
    this.routeList = [...this.routeList, ...defs];
    this.changed();
  }

  registerEntityAction(action: EntityAction): void {
    string(action.id, "entity action id");
    invariant(ENTITY_KINDS.has(action.kind), `unknown entity kind ${String(action.kind)}`);
    if (this.actionList.some((a) => a.id === action.id)) throw new Error(`Entity action ${action.id} is already registered`);
    this.actionList = [...this.actionList, action];
    this.changed();
  }

  /** A stable array that changes identity only when a registration adds to it. */
  routes = (): readonly RouteDef[] => this.routeList;

  actions = (): readonly EntityAction[] => this.actionList;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private changed(): void {
    for (const listener of [...this.listeners]) listener();
  }
}

export const registry = new Registry();

export function registerRoutes(defs: RouteDef[]): void {
  registry.registerRoutes(defs);
}

export function registerEntityAction(action: EntityAction): void {
  registry.registerEntityAction(action);
}
