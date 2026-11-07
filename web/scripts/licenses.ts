// The licence rule of the web workspace (WORKSTREAMS.md A1): every package in
// package-lock.json is MIT, ISC, BSD or Apache-2.0, apart from named dev-only tools.

export const ALLOWED = new Set(["MIT", "ISC", "BSD-2-Clause", "BSD-3-Clause", "0BSD", "Apache-2.0"]);

export interface Exception {
  readonly license: string;
  readonly reason: string;
}

// Build tools only: the rule refuses each of them in a runtime dependency's tree.
export const DEV_ONLY_EXCEPTIONS: Readonly<Record<string, Exception>> = {
  minimatch: {
    license: "BlueOak-1.0.0",
    reason: "permissive; required by eslint and typescript-eslint",
  },
  "caniuse-lite": {
    license: "CC-BY-4.0",
    reason: "browser-support data; eslint-plugin-react-hooks reaches it through @babel/core",
  },
  "axe-core": {
    license: "MPL-2.0",
    reason: "accessibility checks in tests only; the web build refuses any chunk that contains it",
  },
};

interface LockEntry {
  readonly license?: unknown;
  readonly dev?: unknown;
  readonly link?: unknown;
}

function packageName(lockPath: string): string {
  const at = lockPath.lastIndexOf("node_modules/");
  return at === -1 ? lockPath : lockPath.slice(at + "node_modules/".length);
}

/** Whether an SPDX expression of simple `AND` / `OR` terms is acceptable. */
export function licenseAllowed(expression: string): boolean {
  const bare = expression.replace(/^\((.*)\)$/, "$1").trim();
  if (bare.includes("(")) return false;
  const hasAnd = bare.includes(" AND ");
  const hasOr = bare.includes(" OR ");
  if (hasAnd && hasOr) return false;
  if (hasAnd) return bare.split(" AND ").every((term) => ALLOWED.has(term.trim()));
  return bare.split(" OR ").some((term) => ALLOWED.has(term.trim()));
}

type Exceptions = Readonly<Record<string, Exception>>;

function entryProblem(lockPath: string, entry: LockEntry, exceptions: Exceptions, used: Set<string>): string | undefined {
  const name = packageName(lockPath);
  if (typeof entry.license !== "string" || entry.license === "") return `${lockPath}: no licence in the lockfile`;
  if (licenseAllowed(entry.license)) return undefined;
  const exception = exceptions[name];
  if (exception === undefined || exception.license !== entry.license) return `${lockPath}: ${entry.license} is not allowed`;
  if (entry.dev !== true) return `${lockPath}: ${entry.license} is allowed in dev-only tools, not at runtime`;
  used.add(name);
  return undefined;
}

/** Every licence problem in a parsed package-lock.json; empty when the rule holds. */
export function licenseProblems(lock: unknown, exceptions: Exceptions = DEV_ONLY_EXCEPTIONS): string[] {
  const packages = (lock as { packages?: unknown } | null)?.packages;
  if (typeof packages !== "object" || packages === null) return ["package-lock.json has no packages map (lockfile v2 or later)"];
  const problems: string[] = [];
  const used = new Set<string>();
  for (const [lockPath, entry] of Object.entries(packages as Record<string, LockEntry>)) {
    if (lockPath === "" || entry.link === true) continue;
    const problem = entryProblem(lockPath, entry, exceptions, used);
    if (problem !== undefined) problems.push(problem);
  }
  for (const name of Object.keys(exceptions)) {
    if (!used.has(name)) problems.push(`exception for ${name} is unused: remove it from scripts/licenses.ts`);
  }
  return problems;
}
