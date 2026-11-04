// React bindings: one store read through useSyncExternalStore (ARCHITECTURE.md §a).

import type { Capabilities, ConnId, EntityAction, RouteDef } from "@aether-ui/protocol";
import { createContext, createElement, useCallback, useContext, useEffect, useMemo, useSyncExternalStore, type ReactNode } from "react";
import type { App } from "./app.ts";
import type { Json } from "./guards.ts";
import { currentHash, matchRoute, onHashChange, type RouteMatch } from "./router.ts";
import { aetherAccess, archonAccess, checkAllowed, type Access } from "./commands.ts";
import { errorText } from "./guards.ts";
import type { ReadEntry } from "./reads.ts";
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

/**
 * Rereads `command` into the store under `key` while `conn` is ready, and again whenever `refresh`
 * changes; returns the stored entry, so the screen derives from the store and keeps no copy.
 * A null command reads nothing (the screen lacks what the read needs, or may not send it).
 */
/** A command as a builder in `@aether-ui/protocol` makes it, or as a plain record. */
export type CommandInput = Json | { readonly cmd: string };

export function useReread(conn: ConnId, key: string, command: CommandInput | null, tags: readonly string[], refresh: unknown = 0): ReadEntry | undefined {
  const app = useApp();
  const ready = useSelector(useCallback((state: State) => state.connections[conn]?.state === "ready", [conn]));
  const text = command === null ? null : JSON.stringify(command);
  const tagText = tags.join(" ");
  useEffect(() => {
    if (!ready || text === null) return;
    void app.reread(conn, key, JSON.parse(text) as Json, tagText.split(" "));
  }, [app, conn, key, ready, text, tagText, refresh]);
  return useSelector(useCallback((state: State) => state.reads[conn]?.[key], [conn, key]));
}

export interface Permission {
  readonly access: Access;
  /** Why this device may not send the command now, shown beside the disabled control; null when it may. */
  readonly refusal: string | null;
}

/** What sending `command` on `conn` needs, and why it is refused now: the same check `useCommand` makes. */
export function permissionFor(state: State, conn: ConnId, input: CommandInput): Permission {
  const command = input as Json;
  const info = state.connections[conn];
  const access = info?.daemon === "archon" ? archonAccess(command) : aetherAccess(command);
  if (!info || info.state !== "ready") return { access, refusal: `${info?.label ?? "The daemon"} is not connected` };
  try {
    checkAllowed(access, info.standing);
    return { access, refusal: null };
  } catch (error) {
    return { access, refusal: errorText(error) };
  }
}

export function usePermission(conn: ConnId, command: CommandInput): Permission {
  const text = JSON.stringify(command);
  const select = useCallback((state: State) => permissionFor(state, conn, JSON.parse(text) as Json), [conn, text]);
  return useSelector(select, samePermission);
}

const samePermission = (a: Permission, b: Permission): boolean => a.access === b.access && a.refusal === b.refusal;
