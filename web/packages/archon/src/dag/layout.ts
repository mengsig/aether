// The DAG's layout, derived from its structure alone (CONTRACTS.md §5.4). The layout function runs
// again only when the structure key changes; a status change keeps the layout it has.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { useEffect, useRef, useState } from "react";
import { invariant } from "../queue/model.ts";

export type LayoutFn = (model: GraphModel) => Promise<Layout>;

export interface PlacedDag {
  /** The newest layout computed; while a new structure computes, the previous one stays on screen. */
  readonly layout: Layout | undefined;
  /** True while the layout of the current structure is still computing. */
  readonly pending: boolean;
  readonly error: string | null;
}

interface Settled { readonly key: string; readonly layout?: Layout; readonly error?: string }

export function useDagLayout(structure: { readonly key: string; readonly model: GraphModel }, layout: LayoutFn): PlacedDag {
  invariant(structure.key.length > 0, "a structure has a key");
  const [settled, setSettled] = useState<Settled | null>(null);
  const [last, setLast] = useState<Layout | undefined>(undefined);
  const model = useRef(structure.model);
  useEffect(() => { model.current = structure.model; });
  const { key } = structure;
  useEffect(() => {
    let current = true;
    layout(model.current).then(
      (placed) => {
        invariant(placed.positions instanceof Map, "a layout maps node ids to positions");
        if (!current) return;
        setSettled({ key, layout: placed });
        setLast(placed);
      },
      (failure: unknown) => { if (current) setSettled({ key, error: failure instanceof Error ? failure.message : String(failure) }); },
    );
    return () => { current = false; };
  }, [key, layout]);
  const mine = settled?.key === key ? settled : null;
  return { layout: mine?.layout ?? last, pending: mine === null, error: mine?.error ?? null };
}
