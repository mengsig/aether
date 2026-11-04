import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { usageFacts } from "./facts.ts";

const usage = (overrides: Partial<aether.Usage>): aether.Usage => ({
  input_tokens: 1_000, output_tokens: 200, cache_read_tokens: 300, cache_write_tokens: 100, cost_usd: 0.01, cost_complete: true, cost_basis: "provider_reported",
  ...overrides,
});

describe("usage facts", () => {
  test("count the 1-hour cache writes once, as the part of cache_write_tokens they are", () => {
    const cache = usageFacts(usage({ cache_write_1h_tokens: 100 })).find((fact) => fact.label === "Cache");
    expect(cache?.value).toBe("300 read · 100 written");
  });

  test("say nothing was spent when the agent has no usage yet", () => {
    expect(usageFacts(undefined)).toEqual([{ label: "Cost", value: "Nothing spent yet" }]);
  });
});
