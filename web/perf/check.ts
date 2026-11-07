// Compares measured values with perf/budgets.json and fails when one regresses more than the
// tolerance (10%) over its budget. `--ci` checks only the measures that hold on any host.

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

export type Enforce = "ci" | "local";

export interface Budget {
  readonly budget: number;
  readonly unit: string;
  /** `ci`: the same on every host (counts, bytes); `local`: wall time or heap, checked only on the host it was set on. */
  readonly enforce: Enforce;
  readonly fixture: string;
  /** The median of the calibration runs that set the budget. */
  readonly measured: number;
  /** How far apart calibration runs landed, as a fraction; 0 for deterministic measures. */
  readonly noise: number;
  readonly reason: string;
}

export interface Budgets {
  readonly tolerance: number;
  readonly host: string;
  readonly measures: Readonly<Record<string, Budget>>;
}

export type Results = Readonly<Record<string, { readonly value: number }>>;

export interface Verdict {
  readonly name: string;
  readonly value: number | null;
  readonly budget: number;
  readonly limit: number;
  readonly status: "ok" | "over" | "missing" | "skipped";
}

export function validateBudgets(raw: unknown): Budgets {
  const budgets = raw as Budgets;
  if (typeof budgets !== "object" || budgets === null) throw new Error("budgets.json is not an object");
  if (!(budgets.tolerance > 0 && budgets.tolerance < 1)) throw new Error(`tolerance ${budgets.tolerance} is not a fraction`);
  const names = Object.keys(budgets.measures ?? {});
  if (names.length === 0) throw new Error("budgets.json lists no measures");
  for (const name of names) {
    const b = budgets.measures[name] as Budget;
    if (!(Number.isFinite(b.budget) && b.budget >= 0)) throw new Error(`${name}: budget ${b.budget} is not a number`);
    if (b.enforce !== "ci" && b.enforce !== "local") throw new Error(`${name}: enforce is ci or local, not ${String(b.enforce)}`);
    if (!(b.noise >= 0) || !(b.measured >= 0)) throw new Error(`${name}: measured and noise are not numbers`);
    if (b.budget + 1e-9 < b.measured) throw new Error(`${name}: budget ${b.budget} is under its own measurement ${b.measured}`);
    if (typeof b.reason !== "string" || b.reason.length < 10 || typeof b.fixture !== "string") throw new Error(`${name}: needs a fixture and a reason`);
  }
  return budgets;
}

/** One verdict per budget; with `ciOnly`, local measures are skipped. Measures without a budget fail. */
export function evaluate(budgets: Budgets, results: Results, ciOnly: boolean): { verdicts: Verdict[]; unbudgeted: string[] } {
  const verdicts = Object.entries(budgets.measures).map(([name, b]): Verdict => {
    // A zero budget (a relayout that must never happen) allows nothing over it.
    const limit = b.budget * (1 + budgets.tolerance);
    const value = results[name]?.value ?? null;
    if (ciOnly && b.enforce === "local") return { name, value, budget: b.budget, limit, status: "skipped" };
    if (value === null || !Number.isFinite(value)) return { name, value: null, budget: b.budget, limit, status: "missing" };
    return { name, value, budget: b.budget, limit, status: value > limit ? "over" : "ok" };
  });
  const unbudgeted = Object.keys(results).filter((name) => !(name in budgets.measures)).sort();
  return { verdicts, unbudgeted };
}

export function readResults(dir: string): Results {
  const merged: Record<string, { value: number }> = {};
  const files = readdirSync(dir).filter((file) => file.endsWith(".json")).sort();
  if (files.length === 0) throw new Error(`${dir} holds no results; run the perf suite first`);
  for (const file of files) {
    const measures = JSON.parse(readFileSync(path.join(dir, file), "utf8")) as Results;
    for (const [name, measure] of Object.entries(measures)) {
      if (name in merged) throw new Error(`${name} is measured twice (${file})`);
      merged[name] = { value: measure.value };
    }
  }
  return merged;
}

const shown = (n: number | null): string => (n === null ? "—" : n.toLocaleString("en", { maximumFractionDigits: 2 }));

export function report(verdicts: readonly Verdict[], budgets: Budgets): string {
  const rows = verdicts.map((v) => {
    const b = budgets.measures[v.name] as Budget;
    return `${v.status.padEnd(8)}${v.name.padEnd(40)}${shown(v.value).padStart(14)}${shown(v.budget).padStart(14)}${shown(v.limit).padStart(14)}  ${b.unit} (${b.enforce})`;
  });
  return [`${"status".padEnd(8)}${"measure".padEnd(40)}${"value".padStart(14)}${"budget".padStart(14)}${"limit".padStart(14)}`, ...rows].join("\n");
}

/** Checks one results directory; returns whether every enforced measure held. */
export function check(budgetsFile: string, resultsDir: string, ciOnly: boolean): boolean {
  const budgets = validateBudgets(JSON.parse(readFileSync(budgetsFile, "utf8")));
  const { verdicts, unbudgeted } = evaluate(budgets, readResults(resultsDir), ciOnly);
  console.log(report(verdicts, budgets));
  const failed = verdicts.filter((v) => v.status === "over" || v.status === "missing");
  for (const v of failed) console.error(`perf: ${v.name} ${v.status === "missing" ? "was not measured" : `is ${shown(v.value)}, over its limit ${shown(v.limit)} (budget ${shown(v.budget)} + ${budgets.tolerance * 100}%)`}`);
  for (const name of unbudgeted) console.error(`perf: ${name} has no budget in budgets.json`);
  return failed.length === 0 && unbudgeted.length === 0;
}

if (import.meta.main) {
  const ciOnly = process.argv.includes("--ci");
  const dir = process.argv.slice(2).find((arg) => !arg.startsWith("--")) ?? path.join(import.meta.dirname, ".out");
  if (!check(path.join(import.meta.dirname, "budgets.json"), dir, ciOnly)) process.exit(1);
}
