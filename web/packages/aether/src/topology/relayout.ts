// Asks for a layout only when the visible structure changes (CONTRACTS.md §5.4): an agent added or
// removed, a delegation edge, or a run collapsed or expanded. A status, name or model change reuses it.

import type { GraphModel, Layout } from "@aether-ui/protocol";
import { visibleGraph } from "@aether-ui/graph";
import { invariant } from "../fleet/invariant.ts";

export type LayoutFn = (model: GraphModel) => Promise<Layout>;

export class LayoutGate {
  private readonly compute: LayoutFn;
  private key: string | undefined;
  private current: Promise<Layout> | undefined;
  private count = 0;

  constructor(compute: LayoutFn) {
    invariant(typeof compute === "function", "a layout gate needs a layout function");
    this.compute = compute;
  }

  /** How many layouts the gate has asked for. */
  get requests(): number {
    return this.count;
  }

  /** The layout for the model's structure: the previous promise while the structure is the same. */
  layout(model: GraphModel): Promise<Layout> {
    const { key } = visibleGraph(model);
    if (this.current !== undefined && key === this.key) return this.current;
    this.key = key;
    this.count += 1;
    const pending = this.compute(model);
    invariant(typeof pending?.then === "function", "a layout function returns a promise");
    // A failed layout is forgotten, so the next request for the same structure tries again.
    pending.catch(() => { if (this.current === pending) this.current = undefined; });
    this.current = pending;
    return pending;
  }
}
