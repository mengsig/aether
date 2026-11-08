// Which folded parts of a transcript are open (tool output, reasoning). The set outlives the rows,
// which the virtual list drops when they scroll away, and only the part whose state changed redraws.

import { createContext, useCallback, useContext, useSyncExternalStore } from "react";
import { invariant } from "../fleet/invariant.ts";

type Listener = () => void;

export class ToggleSet {
  readonly #open = new Set<string>();
  readonly #listeners = new Set<Listener>();

  has(key: string): boolean {
    return this.#open.has(key);
  }

  set(key: string, open: boolean): void {
    invariant(key !== "", "a folded part has a key");
    if (this.#open.has(key) === open) return;
    if (open) this.#open.add(key);
    else this.#open.delete(key);
    for (const listener of this.#listeners) listener();
  }

  toggle(key: string): void {
    this.set(key, !this.#open.has(key));
  }

  subscribe = (listener: Listener): (() => void) => {
    this.#listeners.add(listener);
    return () => void this.#listeners.delete(listener);
  };
}

export const ToggleContext = createContext<ToggleSet | null>(null);

/** Whether the folded part under `key` is open, and a way to flip it. */
export function useToggle(key: string): readonly [boolean, () => void] {
  const toggles = useContext(ToggleContext);
  invariant(toggles !== null, "a folded part sits inside a conversation's ToggleContext");
  const open = useSyncExternalStore(toggles.subscribe, () => toggles.has(key));
  const flip = useCallback(() => toggles.toggle(key), [toggles, key]);
  return [open, flip] as const;
}

/** The toggle key of a tool call: its tool_use id when it has one, else its place in the row. */
export const toolKey = (rowKey: string, index: number, id: string | null): string => (id === null ? `t:${rowKey}:${index}` : `t:${id}`);
export const foldKey = (rowKey: string, index: number): string => `f:${rowKey}:${index}`;
export const orphanKey = (rowKey: string, toolUseId: string): string => `t:orphan:${toolUseId}:${rowKey}`;
