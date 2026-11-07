// Which Noesis settings the sheet hands to the desktop's privileged action, and which stay a change.

import { expect, test } from "vitest";
import { splitHostPatch } from "./data.ts";

test("splitHostPatch moves exactly the host-only keys, as main's flat keys", () => {
  const { host, rest } = splitHostPatch({
    enabled: true, billing: { allow_api: false }, prompts: { overrides: true },
    budget: { background_tokens_daily: 9 }, models: { cheap: "m" }, extraction: { sessions: "auto" },
  });
  expect(host).toEqual({
    "noesis.enabled": true, "noesis.billing.allow_api": false, "noesis.prompts.overrides": true, "noesis.budget.background_tokens_daily": 9,
  });
  expect(rest).toEqual({ models: { cheap: "m" }, extraction: { sessions: "auto" } });
});

test("a patch with no host-only key stays whole", () => {
  expect(splitHostPatch({ models: { strong: null } })).toEqual({ host: {}, rest: { models: { strong: null } } });
});
