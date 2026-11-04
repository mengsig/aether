// React bindings: one store read through useSyncExternalStore (ARCHITECTURE.md §a).

import type { Capabilities, ConnId, EntityAction, RouteDef } from "@aether-ui/protocol";
import { createContext, createElement, useCallback, useContext, useMemo, useSyncExternalStore, type ReactNode } from "react";
import type { App } from "./app.ts";
import type { Json } from "./guards.ts";
import { currentHash, matchRoute, onHashChange, type RouteMatch } from "./router.ts";
import type { State, Store } from "./store.ts";

const AppContext = createContext<App | null>(null);

export function AppProvider(props: { app: App; children?: ReactNode }): ReactNode {
  return createElement(AppContext.Provider, { value: props.app }, props.children);
}

export function useApp(): App {
  const app = useContext(AppContext);
  if (!app) throw new Error("useApp needs an <AppProvider> above it");
  return app;
}

/** Reads one slice of the store; `eq` keeps the previous value when they are equal. */
export function useSelector<T>(sel: (state: State) => T, eq: (a: T, b: T) => boolean = Object.is): T {
  const { store } = useApp();
  const read = useMemo(() => memoizedRead(store, sel, eq), [store, sel, eq]);
  return useSyncExternalStore(store.subscribe, read, read);
}

/** A snapshot reader that recomputes only for a new snapshot and keeps equal results. */
export function memoizedRead<T>(store: Store, sel: (state: State) => T, eq: (a: T, b: T) => boolean): () => T {
  let last: { state: State; value: T } | null = null;
  return () => {
    const state = store.getSnapshot();
    if (last && last.state === state) return last.value;
    const value = sel(state);
    last = { state, value: last && eq(last.value, value) ? last.value : value };
    return last.value;
  };
}

const selectCaps = (state: State): Capabilities => state.capabilities;

export function useCapabilities(): Capabilities {
  return useSelector(selectCaps);
}

/** Sends commands on one connection with idempotency keys, role checks and error reports. */
export function useCommand(conn: ConnId): (command: Json) => Promise<Json> {
  const app = useApp();
  return useCallback((command: Json) => app.send(conn, command), [app, conn]);
}

export function useRoutes(): readonly RouteDef[] {
  const { registry } = useApp();
  return useSyncExternalStore(registry.subscribe, registry.routes, registry.routes);
}

export function useEntityActions(): readonly EntityAction[] {
  const { registry } = useApp();
  return useSyncExternalStore(registry.subscribe, registry.actions, registry.actions);
}

export function useRoute(): RouteMatch | null {
  const routes = useRoutes();
  const hash = useSyncExternalStore(onHashChange, currentHash, () => "#/");
  return useMemo(() => matchRoute(routes, hash), [routes, hash]);
}
