import { describe, expect, test } from "vitest";
import { roleLabel } from "./roles.ts";

describe("roleLabel", () => {
  test("names an Archon run root by its pipeline", () => {
    expect(roleLabel("archon-run:adaptive")).toEqual({ kind: "archon-run", text: "Archon run · adaptive", role: "archon-run:adaptive" });
    expect(roleLabel("archon-run:").text).toBe("Archon run");
  });

  test("names an Archon stage by stage, then pipeline", () => {
    expect(roleLabel("archon-stage:default/review")).toMatchObject({ kind: "archon-stage", text: "Archon review · default" });
    expect(roleLabel("archon-stage:a/b/build").text).toBe("Archon build · a/b");
    expect(roleLabel("archon-stage:build").text).toBe("Archon build");
  });

  test("leaves every other role as it is", () => {
    expect(roleLabel("reviewer.security")).toEqual({ kind: "agent", text: "reviewer.security", role: "reviewer.security" });
    expect(roleLabel("archon").kind).toBe("agent");
    expect(roleLabel("").text).toBe("No role");
  });
});
