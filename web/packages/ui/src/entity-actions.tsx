import { createContext, useContext, useMemo, type ReactNode } from "react";
import type { Capabilities, EntityAction, EntityRef } from "@aether-ui/protocol";
import { invariant } from "./classes.ts";
import { Menu } from "./menu.tsx";

interface ActionContextValue {
  readonly actions: readonly EntityAction[];
  readonly capabilities: Capabilities;
  readonly onError: (action: EntityAction, error: unknown) => void;
}

const ActionContext = createContext<ActionContextValue | null>(null);

export function EntityActionProvider(props: ActionContextValue & { readonly children?: ReactNode }) {
  const { actions, capabilities, onError } = props;
  const value = useMemo(() => ({ actions, capabilities, onError }), [actions, capabilities, onError]);
  return <ActionContext.Provider value={value}>{props.children}</ActionContext.Provider>;
}

/** The registered actions for one entity whose `requires` holds under the current capabilities. */
export function actionsFor(actions: readonly EntityAction[], capabilities: Capabilities, entity: EntityRef): EntityAction[] {
  invariant(entity.id.length > 0 && entity.conn.length > 0, "an entity ref names its connection and id");
  return actions.filter((action) => action.kind === entity.kind && (action.requires?.(capabilities, entity) ?? true));
}

/** Runs an action, sending a thrown or rejected failure to the shell's error report instead of losing it. */
export function runAction(action: EntityAction, entity: EntityRef, onError: ActionContextValue["onError"]): void {
  invariant(action.kind === entity.kind, `action ${action.id} is for ${action.kind}, not ${entity.kind}`);
  try {
    void Promise.resolve(action.run(entity)).catch((error: unknown) => onError(action, error));
  } catch (error) {
    onError(action, error);
  }
}

/** A menu of every action other features registered for this entity (CONTRACTS.md §2.2). */
export function EntityActionMenu({ entity, label = "Actions" }: { readonly entity: EntityRef; readonly label?: string }) {
  const context = useContext(ActionContext);
  invariant(context !== null, "EntityActionMenu needs the AppShell (or an EntityActionProvider) above it");
  const available = actionsFor(context.actions, context.capabilities, entity);
  if (available.length === 0) return null;
  const items = available.map((action) => ({ id: action.id, label: action.label, onSelect: () => runAction(action, entity, context.onError) }));
  return <Menu label={label} items={items} variant="quiet" />;
}
