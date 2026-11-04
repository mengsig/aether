import { describe, expect, test } from "vitest";
import { costCaveat, formatBytes, formatCost, formatDuration, formatTokens } from "./format.ts";

const usage = { input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 1.234, cost_complete: true } as const;

describe("formatting", () => {
  test("cost marks estimates and gaps", () => {
    expect(formatCost(usage)).toBe("$1.23");
    expect(formatCost({ ...usage, cost_usd: 0.00042, cost_basis: "list_price" })).toBe("≈$0.0004");
    expect(formatCost({ ...usage, cost_complete: false })).toBe("$1.23+");
    expect(costCaveat({ ...usage, cost_complete: false, cost_gap: "unpriced" })).toBe("Leaves out some work: a model has no configured price");
    expect(costCaveat(usage)).toBeNull();
    expect(() => formatCost({ ...usage, cost_usd: -1 })).toThrow();
  });

  test("tokens, bytes and durations", () => {
    expect([formatTokens(999), formatTokens(1_250), formatTokens(88_402), formatTokens(3_900_120)]).toEqual(["999", "1.3k", "88k", "3.9M"]);
    expect([formatBytes(512), formatBytes(48_213), formatBytes(5_242_880)]).toEqual(["512 B", "47.1 KB", "5.0 MB"]);
    expect([formatDuration(420), formatDuration(4_200), formatDuration(125_000)]).toEqual(["420 ms", "4.2 s", "2 min 5 s"]);
  });
});
