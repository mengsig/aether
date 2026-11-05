// Memory scopes in words, and the CLI's scope syntax (crates/aether/src/cli/memory.rs) for the
// tools' optional scope field. A blank field means every scope.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";

export const SCOPE_HINT = "Blank for every scope; or global, repo:<id>, role:<role>, repo-role:<role>@<id>, agent:<id>, subtree:<id>";

/** "Global", or the repository a memory belongs to, in words. */
export function scopeText(scope: aether.MemoryScope): string {
  switch (scope.scope) {
    case "global": return "Global";
    case "repository": return `Repository ${scope.id}`;
    case "role": return `Role ${scope.role}`;
    case "agent": return `Agent ${scope.id}`;
    case "repository_role": return `Repository ${scope.id} · role ${scope.role}`;
    case "subtree": return `Subtree of ${scope.root}`;
  }
}

export type ParsedScope = { readonly scope: aether.MemoryScope | null } | { readonly error: string };

const PREFIXED: Readonly<Record<string, (rest: string) => aether.MemoryScope>> = {
  "role:": (role) => ({ scope: "role", role }),
  "agent:": (id) => ({ scope: "agent", id }),
  "subtree:": (root) => ({ scope: "subtree", root }),
  "repo:": (id) => ({ scope: "repository", id }),
};

/** The scope a tool runs over, as the CLI reads it: null for every scope, or what the text lacks. */
export function parseScope(text: string): ParsedScope {
  const value = text.trim();
  if (value === "") return { scope: null };
  if (value.toLowerCase() === "global") return { scope: { scope: "global" } };
  if (value.startsWith("repo-role:")) return repositoryRole(value.slice("repo-role:".length));
  const prefix = Object.keys(PREFIXED).find((key) => value.startsWith(key));
  const rest = prefix === undefined ? value : value.slice(prefix.length).trim();
  if (rest === "") return { error: `Name what ${prefix ?? "the scope"} refers to` };
  const build = PREFIXED[prefix ?? "repo:"];
  invariant(build !== undefined, "every prefix has a builder");
  return { scope: build(rest) };
}

/** `repo-role:<role>@<repository>`: a role family never holds `@`, so the first one ends it. */
function repositoryRole(rest: string): ParsedScope {
  const at = rest.indexOf("@");
  const role = at < 0 ? "" : rest.slice(0, at).trim();
  const id = at < 0 ? "" : rest.slice(at + 1).trim();
  if (role === "" || id === "") return { error: "Use repo-role:<role>@<repository>" };
  invariant(!role.includes("@"), "the role ends at the first @");
  return { scope: { scope: "repository_role", id, role } };
}
