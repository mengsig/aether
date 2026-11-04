// The DAG's layout, derived from its structure alone (CONTRACTS.md §5.4). The layout function runs
// again only when the structure key changes; a status change keeps the layout it has.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { useCallback, useEffect, useRef, useState } from "react";
import { invariant } from "../queue/model.ts";

export type LayoutFn = (model: GraphModel) => Promise<Layout>;

export interface PlacedDag {
  /** The newest layout computed; while a new structure computes, the previous one stays on screen. */
  readonly layout: Layout | undefined;
  /** True while the layout of the current structure is still computing. */
  readonly pending: boolean;
  /** Why the current structure could not be laid out; the previous layout, if any, stays on screen. */
  readonly error: string | null;
  /** Lays the current structure out again after a failure. */
  readonly retry: () => void;
}

interface Settled { readonly key: string; readonly attempt: number; readonly layout?: Layout; readonly error?: string }

export function useDagLayout(structure: { readonly key: string; readonly model: GraphModel }, layout: LayoutFn): PlacedDag {
  invariant(structure.key.length > 0, "a structure has a key");
  const [settled, setSettled] = useState<Settled | null>(null);
  const [last, setLast] = useState<Layout | undefined>(undefined);
  const [attempt, setAttempt] = useState(0);
  const model = useRef(structure.model);
  useEffect(() => { model.current = structure.model; });
  const { key } = structure;
  useEffect(() => {
    let current = true;
    layout(model.current).then(
      (placed) => {
        invariant(placed.positions instanceof Map, "a layout maps node ids to positions");
        if (!current) return;
        setSettled({ key, attempt, layout: placed });
        setLast(placed);
      },
      (failure: unknown) => { if (current) setSettled({ key, attempt, error: failure instanceof Error ? failure.message : String(failure) }); },
    );
    return () => { current = false; };
  }, [key, layout, attempt]);
  const retry = useCallback(() => { setAttempt((n) => n + 1); }, []);
  const mine = settled?.key === key && settled.attempt === attempt ? settled : null;
  invariant(mine === null || (mine.layout === undefined) !== (mine.error === undefined), "a settled layout either placed or failed");
  return { layout: mine?.layout ?? last, pending: mine === null, error: mine?.error ?? null, retry };
}
