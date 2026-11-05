import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, test } from "vitest";
import { evaluate, validateBudgets, type Budget, type Budgets } from "./check.ts";

const budget = (value: number, enforce: Budget["enforce"] = "ci"): Budget => ({
  budget: value, unit: "rows", enforce, fixture: "test", measured: value, noise: 0, reason: "a budget under test",
});
const budgets = (measures: Record<string, Budget>): Budgets => ({ tolerance: 0.1, host: "test", measures });

describe("the perf check", () => {
  test("passes up to 10% over a budget and fails past it", () => {
    const b = budgets({ "a.rows": budget(100) });
    expect(evaluate(b, { "a.rows": { value: 110 } }, false).verdicts[0]?.status).toBe("ok");
    expect(evaluate(b, { "a.rows": { value: 111 } }, false).verdicts[0]?.status).toBe("over");
  });

  test("allows nothing over a zero budget", () => {
    const b = budgets({ "dag.delta.layouts": budget(0) });
    expect(evaluate(b, { "dag.delta.layouts": { value: 0 } }, false).verdicts[0]?.status).toBe("ok");
    expect(evaluate(b, { "dag.delta.layouts": { value: 1 } }, false).verdicts[0]?.status).toBe("over");
  });

  test("fails a budget that was not measured and a measure without a budget", () => {
    const { verdicts, unbudgeted } = evaluate(budgets({ "a.rows": budget(5) }), { "b.rows": { value: 1 } }, false);
    expect(verdicts[0]?.status).toBe("missing");
    expect(unbudgeted).toEqual(["b.rows"]);
  });

  test("skips local measures under --ci, and checks them otherwise", () => {
    const b = budgets({ "a.ms": budget(10, "local") });
    expect(evaluate(b, { "a.ms": { value: 99 } }, true).verdicts[0]?.status).toBe("skipped");
    expect(evaluate(b, { "a.ms": { value: 99 } }, false).verdicts[0]?.status).toBe("over");
  });

  test("refuses a budget set under its own measurement or without a reason", () => {
    expect(() => validateBudgets(budgets({ "a.rows": { ...budget(10), measured: 12 } }))).toThrow(/under its own measurement/);
    expect(() => validateBudgets(budgets({ "a.rows": { ...budget(10), reason: "" } }))).toThrow(/reason/);
  });

  test("accepts the committed budgets, with deterministic measures at their measured value", () => {
    const committed = validateBudgets(JSON.parse(readFileSync(path.join(import.meta.dirname, "budgets.json"), "utf8")));
    expect(committed.tolerance).toBe(0.1);
    for (const [name, b] of Object.entries(committed.measures)) {
      if (b.enforce === "ci") expect([name, b.budget, b.noise]).toEqual([name, b.measured, 0]);
      else expect([name, b.noise > 0]).toEqual([name, true]);
    }
  });
});
