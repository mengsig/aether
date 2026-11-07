// Archon names its agents by role: `archon-run:<pipeline>` for a run's root and
// `archon-stage:<pipeline>/<stage>` for a stage. The label comes from the role alone, so
// Archon-run agents read the same whether archond is connected or not (ARCHITECTURE.md §b).

import { invariant } from "./invariant.ts";

export type RoleKind = "archon-run" | "archon-stage" | "agent";

export interface RoleLabel {
  readonly kind: RoleKind;
  /** The short text a list or a graph node shows. */
  readonly text: string;
  /** The role exactly as the daemon sent it. */
  readonly role: string;
}

const RUN_PREFIX = "archon-run:";
const STAGE_PREFIX = "archon-stage:";

export function roleLabel(role: string): RoleLabel {
  invariant(typeof role === "string", "an agent role is a string");
  if (role.startsWith(RUN_PREFIX)) {
    const pipeline = role.slice(RUN_PREFIX.length);
    return { kind: "archon-run", text: pipeline === "" ? "Archon run" : `Archon run · ${pipeline}`, role };
  }
  if (role.startsWith(STAGE_PREFIX)) {
    const rest = role.slice(STAGE_PREFIX.length);
    const slash = rest.lastIndexOf("/");
    const stage = slash < 0 ? rest : rest.slice(slash + 1);
    const pipeline = slash < 0 ? "" : rest.slice(0, slash);
    const text = stage === "" ? "Archon stage" : pipeline === "" ? `Archon ${stage}` : `Archon ${stage} · ${pipeline}`;
    return { kind: "archon-stage", text, role };
  }
  return { kind: "agent", text: role === "" ? "No role" : role, role };
}
