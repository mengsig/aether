// The messages between the layout engine and its worker. The worker side is `runRequest`,
// kept free of DOM and React so the worker bundle stays small.

import { force } from "./force.ts";
import { layered, type LayoutInput } from "./layered.ts";

export type Algorithm = "layered" | "force";
export interface LayoutRequest extends LayoutInput { id: number; algorithm: Algorithm }
export type LayoutReply =
  | { id: number; ok: true; xs: Float64Array; ys: Float64Array }
  | { id: number; ok: false; error: string };

export function runRequest(request: LayoutRequest): LayoutReply {
  try {
    const run = request.algorithm === "layered" ? layered : request.algorithm === "force" ? force : undefined;
    if (run === undefined) return { id: request.id, ok: false, error: `unknown layout algorithm ${String(request.algorithm)}` };
    const { xs, ys } = run(request);
    return { id: request.id, ok: true, xs, ys };
  } catch (error) {
    // The engine rejects the caller's promise with this message; the worker itself stays usable.
    return { id: request.id, ok: false, error: error instanceof Error ? error.message : String(error) };
  }
}
