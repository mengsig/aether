// Generated from clients/protocol.schema.json by scripts/generate_clients.py.
// Do not edit: change the Rust types and regenerate.

import type {
  PlanEdge,
  PlanNode,
} from "../../protocol.ts";

export interface PlanTree {
  "edges": Array<PlanEdge>;
  "nodes": Array<PlanNode>;
  "revision": number;
}
