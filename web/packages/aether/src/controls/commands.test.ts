import { describe, expect, test } from "vitest";
import { DraftError, ruleCommand, type RuleDraft } from "./commands.ts";

const draft: RuleDraft = { scope: "agent", tool: "", argument: "", resource: "", note: "", decision: "denied" };

describe("ruleCommand", () => {
  test("empty matchers cover every tool without inventing a wildcard", () => {
    const command = ruleCommand("agt_test", draft);
    expect(command).toEqual({ cmd: "add_approval_rule", scope: { scope: "agent", id: "agt_test" }, decision: "denied", match: {} });
    expect(command).not.toHaveProperty("note");
  });

  test("run scope uses id and retains every nonblank matcher and note", () => {
    const command = ruleCommand("agt_test", { ...draft, scope: "run", run: "run_test", tool: " shell ", argument: " git status ", resource: " /repo/ ", note: " Inspect only " });
    expect(command).toEqual({
      cmd: "add_approval_rule", scope: { scope: "run", id: "run_test" }, decision: "denied",
      match: { tool: "shell", argument: "git status", resource_prefix: "/repo/" }, note: "Inspect only",
    });
    expect(command.scope).not.toHaveProperty("run");
  });

  test("a blank tool with a resource prefix and note is a valid deny rule", () => {
    const command = ruleCommand("agt_test", { ...draft, tool: " ", resource: "/private/", note: "Keep private" });
    expect(command.match).toEqual({ resource_prefix: "/private/" });
    expect(command.note).toBe("Keep private");
    expect(command.decision).toBe("denied");
  });

  test.each([undefined, null, "", "  "])("run scope rejects a missing run (%s)", (run) => {
    const build = () => ruleCommand("agt_test", { ...draft, scope: "run", ...(run === undefined ? {} : { run }) });
    expect(build).toThrow(DraftError);
    expect(build).toThrow("This agent has no run");
  });
});
