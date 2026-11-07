// The hash router (CONTRACTS.md §2.3): Aether's listener mounts `/archon/*`
// and Electron serves `app://`, so routes live in the fragment.

import type { RouteDef } from "@aether-ui/protocol";
import { invariant } from "./guards.ts";

export interface RouteMatch {
  readonly route: RouteDef;
  readonly params: Readonly<Record<string, string>>;
}

/** A typed-in hash may carry a broken escape; it matches nothing rather than throwing. */
function decode(value: string): string | null {
  try {
    return decodeURIComponent(value);
  } catch (error) {
    if (error instanceof URIError) return null;
    throw error;
  }
}

function segments(path: string): string[] {
  return path.replace(/^#?\/?/, "").split("/").filter((part) => part.length > 0);
}

/** Matches one route pattern; `:name` takes a segment, a final `*` takes the rest as `params["*"]`. */
export function matchPattern(pattern: string, path: string): Record<string, string> | null {
  const want = segments(pattern);
  const have = segments(path.split("?")[0] ?? "");
  const params: Record<string, string> = {};
  for (const [index, part] of want.entries()) {
    if (part === "*") {
      invariant(index === want.length - 1, `* ends the pattern ${pattern}`);
      params["*"] = have.slice(index).join("/");
      return params;
    }
    const value = have[index];
    if (value === undefined) return null;
    if (part.startsWith(":")) {
      const decoded = decode(value);
      if (decoded === null) return null;
      params[part.slice(1)] = decoded;
    } else if (part !== value) return null;
  }
  return have.length === want.length ? params : null;
}

/** More literal segments first, then fewer params, so `#/fleet/topology` beats `#/fleet/:x`. */
function specificity(pattern: string): number {
  return segments(pattern).reduce((score, part) => score + (part === "*" ? 0 : part.startsWith(":") ? 1 : 3), 0);
}

export function matchRoute(routes: readonly RouteDef[], hash: string): RouteMatch | null {
  const ordered = [...routes].sort((a, b) => specificity(b.path) - specificity(a.path));
  for (const route of ordered) {
    const params = matchPattern(route.path, hash);
    if (params) return { route, params };
  }
  return null;
}

/** Builds `#/agents/<conn>/<agent>` from a pattern, encoding every value. */
export function routeHref(pattern: string, params: Readonly<Record<string, string>> = {}): string {
  const path = segments(pattern).map((part) => {
    if (!part.startsWith(":")) return part;
    const value = params[part.slice(1)];
    if (value === undefined) throw new Error(`Route ${pattern} needs ${part}`);
    return encodeURIComponent(value);
  });
  return `#/${path.join("/")}`;
}

export function currentHash(): string {
  return globalThis.location?.hash || "#/";
}

export function navigate(href: string): void {
  invariant(href.startsWith("#/"), `a route starts with #/, got ${href}`);
  globalThis.location.hash = href;
}

export function onHashChange(listener: () => void): () => void {
  globalThis.addEventListener("hashchange", listener);
  return () => globalThis.removeEventListener("hashchange", listener);
}
