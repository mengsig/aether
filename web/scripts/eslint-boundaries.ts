// The import rules of CONTRACTS.md §2.2 as one ESLint rule. A file under
// packages/<name>/ may import only what its package's row below allows.

import path from "node:path";
import type { Rule } from "eslint";

interface Allowance {
  readonly packages: readonly string[];
  readonly external: readonly string[];
  /** Packages it may import only with `import type`. */
  readonly typeOnly: readonly string[];
}

const REACT = ["react", "react-dom"];
const FEATURE: Allowance = { packages: ["core", "ui", "graph", "protocol"], external: REACT, typeOnly: [] };

export const BOUNDARIES: Readonly<Record<string, Allowance>> = {
  protocol: { packages: [], external: [], typeOnly: [] },
  core: { packages: ["protocol"], external: REACT, typeOnly: [] },
  ui: { packages: ["protocol"], external: REACT, typeOnly: ["protocol"] },
  // The knowledge graph's force layout is d3-force (ARCHITECTURE.md §a).
  graph: { packages: ["protocol"], external: [...REACT, "d3-force"], typeOnly: ["protocol"] },
  aether: FEATURE,
  archon: FEATURE,
  github: FEATURE,
  noesis: FEATURE,
  settings: FEATURE,
};

// Tests may also use the test runner, Node and the TypeScript compiler API.
const TEST_EXTERNAL = ["vitest", "node:*", "typescript"];
const SCOPE = "@aether-ui/";

type Target = { kind: "package"; name: string } | { kind: "external"; name: string } | { kind: "error"; message: string };

function packageOf(root: string, file: string): string | undefined {
  const parts = path.relative(root, file).split(path.sep);
  return parts[0] === "packages" && parts.length > 2 ? parts[1] : undefined;
}

function externalName(specifier: string): string {
  if (specifier.startsWith("node:")) return "node:*";
  const parts = specifier.split("/");
  return (specifier.startsWith("@") ? parts.slice(0, 2) : parts.slice(0, 1)).join("/");
}

function resolveTarget(root: string, file: string, own: string, specifier: string): Target {
  if (specifier.startsWith(SCOPE)) {
    const name = specifier.slice(SCOPE.length);
    if (name.includes("/")) return { kind: "error", message: `import ${SCOPE}${name.split("/")[0]} itself, not a file inside it` };
    return { kind: "package", name };
  }
  if (!specifier.startsWith(".")) return { kind: "external", name: externalName(specifier) };
  const target = packageOf(root, path.resolve(path.dirname(file), specifier));
  if (target === own) return { kind: "package", name: own };
  const through = target === undefined ? "" : `; import ${SCOPE}${target} instead`;
  return { kind: "error", message: `a relative import must stay inside packages/${own}${through}` };
}

function isTypeOnly(node: Rule.Node): boolean {
  const declaration = node as unknown as { importKind?: string; exportKind?: string; specifiers?: Array<{ importKind?: string }> };
  if (declaration.importKind === "type" || declaration.exportKind === "type") return true;
  const specifiers = declaration.specifiers ?? [];
  return specifiers.length > 0 && specifiers.every((specifier) => specifier.importKind === "type");
}

function problem(own: string, allowance: Allowance, target: Target, node: Rule.Node, isTest: boolean): string | undefined {
  if (target.kind === "error") return target.message;
  if (target.kind === "external") {
    const allowed = [...allowance.external, ...(isTest ? TEST_EXTERNAL : [])];
    return allowed.includes(target.name) ? undefined : `packages/${own} must not import ${target.name} (CONTRACTS.md §2.2)`;
  }
  if (target.name === own) return undefined;
  if (!(target.name in BOUNDARIES)) return `${SCOPE}${target.name} is not a package of CONTRACTS.md §2.1`;
  if (!allowance.packages.includes(target.name)) {
    const why = allowance === FEATURE && BOUNDARIES[target.name] === FEATURE
      ? "; feature packages cross over only through routes and the entity-action registry" : "";
    return `${SCOPE}${own} must not import ${SCOPE}${target.name} (CONTRACTS.md §2.2)${why}`;
  }
  if (allowance.typeOnly.includes(target.name) && !isTypeOnly(node)) {
    return `${SCOPE}${own} may import only types from ${SCOPE}${target.name}: use \`import type\``;
  }
  return undefined;
}

const rule: Rule.RuleModule = {
  meta: {
    type: "problem",
    docs: { description: "Enforce the package import rules of CONTRACTS.md §2.2" },
    schema: [{ type: "object", properties: { root: { type: "string" } }, required: ["root"], additionalProperties: false }],
  },
  create(context) {
    const root = (context.options[0] as { root: string }).root;
    const own = packageOf(root, context.filename);
    if (own === undefined) return {};
    const allowance = BOUNDARIES[own];
    if (allowance === undefined) {
      return { Program: (node) => context.report({ node, message: `packages/${own} is not a package of CONTRACTS.md §2.1` }) };
    }
    const isTest = /\.test\.tsx?$/.test(context.filename) || context.filename.includes(`${path.sep}test${path.sep}`);
    const check = (node: Rule.Node, source: unknown) => {
      if (typeof source !== "string") return;
      const message = problem(own, allowance, resolveTarget(root, context.filename, own, source), node, isTest);
      if (message !== undefined) context.report({ node, message });
    };
    return {
      ImportDeclaration: (node) => check(node, node.source.value),
      ExportNamedDeclaration: (node) => check(node, node.source?.value),
      ExportAllDeclaration: (node) => check(node, node.source.value),
      ImportExpression: (node) => {
        if (node.source.type === "Literal") return check(node, node.source.value);
        context.report({ node, message: "a dynamic import needs a string literal, so its package boundary can be checked" });
      },
    };
  },
};

export default { rules: { "import-boundaries": rule } };
