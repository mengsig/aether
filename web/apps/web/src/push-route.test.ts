import { describe, expect, test } from "vitest";
import { pushRoute } from "./push-route.ts";

describe("pushRoute", () => {
  test("maps every route aetherd writes to a route of this app", () => {
    expect(pushRoute("#attention?item=apr_01J0B3CONTROLS0000000001")).toBe("#/inbox");
    expect(pushRoute("#attention?item=question-12")).toBe("#/inbox");
    expect(pushRoute("#agent/agt_01J0B3ABCDEFGH0000000000XY")).toBe("#/inbox");
    expect(pushRoute("#activity")).toBe("#/activity");
  });

  test("keeps a route already in this app's grammar and drops anything else", () => {
    expect(pushRoute("#/fleet")).toBe("#/fleet");
    expect(pushRoute("https://evil.example/#/inbox")).toBeNull();
    expect(pushRoute("#agent/../../x")).toBeNull();
    expect(pushRoute("#elsewhere")).toBeNull();
    expect(pushRoute(undefined)).toBeNull();
  });
});
