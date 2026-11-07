// What paid for model use, in words, including from a daemon that reports no billing split.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { billingWords } from "./model.ts";

const used = (cost: number): aether.Usage =>
  ({ input_tokens: 100, output_tokens: 20, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: cost }) as aether.Usage;
const none = (): aether.Usage => ({ input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0, cost_usd: 0 }) as aether.Usage;

describe("billing words", () => {
  test("subscription use never reads as a charge, and API use names its bill", () => {
    expect(billingWords({ subscription: used(0.02), api: none() })).toBe("Ran on your subscription — no per-request charge (≈$0.02 at API list price).");
    expect(billingWords({ subscription: none(), api: used(1.5) })).toBe("$1.50 billed to your API key.");
  });

  test("use on a removed provider reads as unknown, never as an API charge", () => {
    expect(billingWords({ subscription: none(), api: none(), unknown: used(0.3) })).toBe("Some ran on a provider no longer configured, so what paid for it is unknown.");
    expect(billingWords({ subscription: used(0.02), api: none(), unknown: used(0.3) })).toBe(
      "Ran on your subscription — no per-request charge (≈$0.02 at API list price); some ran on a provider no longer configured, so what paid for it is unknown.",
    );
  });

  test("a daemon without the split says it does not report it, rather than no use", () => {
    expect(billingWords({})).toBe("This daemon does not report what paid for its model use.");
    expect(billingWords(null)).toBe("No model use recorded yet.");
  });
});
