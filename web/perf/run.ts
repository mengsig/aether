// Runs the perf suite and checks it against perf/budgets.json:
//   node perf/run.ts [--ci] [--runs N] [--calibrate]
// Each run writes perf/.out/run-<i>/; --ci checks only host-independent measures; --runs N repeats the
// suite and reports how far runs land apart; --calibrate prints the budgets those runs support.

import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { bundleMeasures } from "./bundle.ts";
import { check, readResults, validateBudgets, type Budget, type Budgets, type Results } from "./check.ts";

const HERE = import.meta.dirname;
const WEB = path.resolve(HERE, "..");
const OUT = path.join(HERE, ".out");
const BUDGETS = path.join(HERE, "budgets.json");

const { values } = parseArgs({
  options: { ci: { type: "boolean", default: false }, runs: { type: "string", default: "1" }, calibrate: { type: "boolean", default: false } },
});
const runs = Number(values.runs);
if (!Number.isInteger(runs) || runs < 1 || runs > 10) throw new Error(`--runs takes 1 to 10, not ${values.runs}`);

function runSuite(dir: string): void {
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  const vitest = path.join(WEB, "node_modules", "vitest", "vitest.mjs");
  const suite = spawnSync(process.execPath, [vitest, "run", "--config", path.join(HERE, "vitest.config.ts")], { cwd: WEB, stdio: "inherit", env: { ...process.env, PERF_OUT: dir } });
  if (suite.status !== 0) throw new Error(`the perf scenarios failed (exit ${suite.status ?? suite.signal})`);
  writeFileSync(path.join(dir, "bundle.json"), `${JSON.stringify(bundleMeasures(path.join(WEB, "dist")), null, 2)}\n`);
}

/** Median and spread ((max - min) / median) of one measure across runs. */
function across(all: readonly Results[], name: string): { median: number; spread: number; values: number[] } {
  const seen = all.map((results) => results[name]?.value).filter((value): value is number => value !== undefined);
  if (seen.length !== all.length) throw new Error(`${name} is missing from a run`);
  const sorted = [...seen].sort((a, b) => a - b);
  const median = sorted[Math.floor(sorted.length / 2)] ?? 0;
  return { median, spread: median > 0 ? ((sorted.at(-1) ?? 0) - (sorted[0] ?? 0)) / median : 0, values: seen };
}

/** Rounds up to the next 5%, so a margin is never tighter than what the runs showed. */
const marginOf = (spread: number): number => Math.ceil((spread + 1e-9) * 20) / 20;

function calibrated(budgets: Budgets, all: readonly Results[]): Budgets {
  const measures: Record<string, Budget> = {};
  for (const [name, held] of Object.entries(budgets.measures)) {
    const { median, spread } = across(all, name);
    // A few runs on a shared host miss its slowest moments: wall time gets at least 20%, heap 10%.
    const noise = held.enforce === "ci" ? 0 : Math.max(held.unit === "ms" ? 0.2 : 0.1, marginOf(spread));
    const budget = held.enforce === "ci" ? median : Number((median * (1 + noise)).toPrecision(4));
    measures[name] = { ...held, measured: Number(median.toPrecision(6)), noise, budget };
  }
  return { ...budgets, measures };
}

const budgets = validateBudgets(JSON.parse(readFileSync(BUDGETS, "utf8")));
const all: Results[] = [];
let passed = true;
for (let run = 1; run <= runs; run += 1) {
  const dir = path.join(OUT, `run-${run}`);
  console.log(`\n==> perf run ${run} of ${runs}${values.ci ? " (ci measures only)" : ""}`);
  runSuite(dir);
  all.push(readResults(dir));
  passed = check(BUDGETS, dir, values.ci) && passed;
}
if (runs > 1) {
  console.log("\n==> across runs: measure, values, spread");
  for (const name of Object.keys(budgets.measures)) {
    const { values: seen, spread } = across(all, name);
    console.log(`${name.padEnd(40)} ${seen.join(" / ").padEnd(40)} ${(spread * 100).toFixed(1)}%`);
  }
}
if (values.calibrate) console.log(`\n==> calibrated budgets\n${JSON.stringify(calibrated(budgets, all), null, 2)}`);
if (!passed) process.exit(1);
