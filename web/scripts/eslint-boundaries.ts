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
  home: FEATURE,
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

/** The value of a string literal or a template literal with no substitutions, else undefined. */
function literalValue(node: unknown): unknown {
  const literal = node as { type?: unknown; value?: unknown; expressions?: unknown[]; quasis?: Array<{ value: { cooked?: unknown } }> } | undefined;
  if (literal?.type === "Literal") return literal.value;
  if (literal?.type !== "TemplateLiteral" || (literal.expressions ?? []).length > 0) return undefined;
  return literal.quasis?.[0]?.value.cooked;
}

function isTypeOnly(node: Rule.Node): boolean {
  type Kinds = { importKind?: string; exportKind?: string };
  const declaration = node as unknown as Kinds & { specifiers?: Kinds[] };
  if (declaration.importKind === "type" || declaration.exportKind === "type") return true;
  const specifiers = declaration.specifiers ?? [];
  return specifiers.length > 0 && specifiers.every((s) => s.importKind === "type" || s.exportKind === "type");
}

function problem(own: string, allowance: Allowance, target: Target, typeOnly: boolean, isTest: boolean): string | undefined {
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
  if (allowance.typeOnly.includes(target.name) && !typeOnly) {
    return `${SCOPE}${own} may import only types from ${SCOPE}${target.name}: use \`import type\``;
  }
  return undefined;
}

/** The patterns of an `import.meta.glob(...)` call, or an error when they are not literals. */
function globPatterns(node: Rule.Node): string[] | string | undefined {
  const call = node as unknown as { callee: { type: string; object?: { type: string; meta?: { name: string } }; property?: { name?: string } }; arguments: unknown[] };
  const callee = call.callee;
  const isGlob = callee.type === "MemberExpression" && callee.object?.type === "MetaProperty"
    && callee.object.meta?.name === "import" && (callee.property?.name ?? "").startsWith("glob");
  if (!isGlob) return undefined;
  if (!globOptionsCheckable(call.arguments[1])) {
    return "import.meta.glob options must be an object literal without `base`, so the matched files stay checkable";
  }
  const first = call.arguments[0] as { type?: string; elements?: unknown[] } | undefined;
  const literals = first?.type === "ArrayExpression" ? first.elements ?? [] : [first];
  const patterns = literals.map(literalValue);
  if (patterns.length === 0 || patterns.some((p) => typeof p !== "string")) {
    return "import.meta.glob needs string literal patterns, so its package boundary can be checked";
  }
  return patterns as string[];
}

/**
 * The path of `new URL(path, import.meta.url)`, which Vite bundles (workers, assets): the path, an
 * error when it is not a static relative one, or undefined for any other `new` expression.
 */
function moduleUrl(node: Rule.Node): { path: string } | { error: string } | undefined {
  const call = node as unknown as { callee: { type: string; name?: string }; arguments: unknown[] };
  if (call.callee.type !== "Identifier" || call.callee.name !== "URL") return undefined;
  const base = call.arguments[1] as { type?: string; object?: { type?: string }; property?: { name?: string } } | undefined;
  if (base?.type !== "MemberExpression" || base.object?.type !== "MetaProperty" || base.property?.name !== "url") return undefined;
  const target = literalValue(call.arguments[0]);
  if (typeof target === "string" && target.startsWith(".")) return { path: target };
  return { error: "new URL(..., import.meta.url) needs a static relative path, so its package boundary can be checked" };
}

// Vite resolves patterns against `base` when given; refusing it keeps each pattern relative to its file.
function globOptionsCheckable(options: unknown): boolean {
  if (options === undefined) return true;
  const object = options as { type?: string; properties?: Array<{ type?: string; computed?: boolean; key?: { name?: unknown; value?: unknown } }> };
  if (object.type !== "ObjectExpression") return false;
  return (object.properties ?? []).every((property) => property.type === "Property" && property.computed !== true
    && (property.key?.name ?? property.key?.value) !== "base");
}

/**
 * The static directory a glob pattern stays under, checked like a relative import. A `..` after the
 * first wildcard could climb out of it, so such a pattern has no safe prefix.
 */
function globPrefix(pattern: string): string | undefined {
  if (!pattern.startsWith("./") && !pattern.startsWith("../")) return undefined;
  const wildcard = pattern.search(/[*?[\]{}()!+@]/);
  if (wildcard === -1) return pattern;
  return pattern.slice(wildcard).includes("..") ? undefined : pattern.slice(0, wildcard);
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
    const check = (node: Rule.Node, source: unknown, typeOnly = isTypeOnly(node)) => {
      if (typeof source !== "string") return;
      const message = problem(own, allowance, resolveTarget(root, context.filename, own, source), typeOnly, isTest);
      if (message !== undefined) context.report({ node, message });
    };
    // TypeScript's own module references; ESTree's listener types do not name them.
    const typescriptNodes: Record<string, (node: Rule.Node) => void> = {
      TSImportType: (node) => check(node, literalValue((node as unknown as { source?: unknown }).source), true),
      TSExternalModuleReference: (node) => check(node, literalValue((node as unknown as { expression?: unknown }).expression)),
    };
    return {
      ImportDeclaration: (node) => check(node, node.source.value),
      ExportNamedDeclaration: (node) => check(node, node.source?.value),
      ExportAllDeclaration: (node) => check(node, node.source.value),
      ImportExpression: (node) => {
        const source = literalValue(node.source);
        if (typeof source === "string") return check(node, source);
        context.report({ node, message: "a dynamic import needs a string literal, so its package boundary can be checked" });
      },
      NewExpression: (node) => {
        const url = moduleUrl(node);
        if (url === undefined) return;
        if ("error" in url) return context.report({ node, message: url.error });
        check(node, url.path, false);
      },
      CallExpression: (node) => {
        const patterns = globPatterns(node);
        if (patterns === undefined) return;
        if (typeof patterns === "string") return context.report({ node, message: patterns });
        for (const pattern of patterns.filter((p) => !p.startsWith("!"))) {
          const prefix = globPrefix(pattern);
          if (prefix === undefined) {
            const message = `import.meta.glob pattern ${JSON.stringify(pattern)} must be relative to this file, with no .. after a wildcard`;
            context.report({ node, message });
            continue;
          }
          check(node, prefix, false);
        }
      },
      ...typescriptNodes,
    };
  },
};

export default { rules: { "import-boundaries": rule } };
