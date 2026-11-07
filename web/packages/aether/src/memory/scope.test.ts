import { expect, test } from "vitest";
import { parseScope } from "./scope.ts";

test("the scope field reads the CLI's syntax", () => {
  expect(parseScope("  ")).toEqual({ scope: null });
  expect(parseScope("Global")).toEqual({ scope: { scope: "global" } });
  expect(parseScope("repo:github.com/mengsig/aether")).toEqual({ scope: { scope: "repository", id: "github.com/mengsig/aether" } });
  expect(parseScope("github.com/mengsig/aether")).toEqual({ scope: { scope: "repository", id: "github.com/mengsig/aether" } });
  expect(parseScope("role:reviewer")).toEqual({ scope: { scope: "role", role: "reviewer" } });
  expect(parseScope("agent:agt_1")).toEqual({ scope: { scope: "agent", id: "agt_1" } });
  expect(parseScope("subtree:agt_1")).toEqual({ scope: { scope: "subtree", root: "agt_1" } });
  expect(parseScope("repo-role:worker.rust@github.com/a/b")).toEqual({ scope: { scope: "repository_role", id: "github.com/a/b", role: "worker.rust" } });
});

test("a scope missing its name is an error, never a guess", () => {
  expect(parseScope("agent:")).toHaveProperty("error");
  expect(parseScope("repo: ")).toHaveProperty("error");
  expect(parseScope("repo-role:worker")).toEqual({ error: "Use repo-role:<role>@<repository>" });
  expect(parseScope("repo-role:@github.com/a/b")).toHaveProperty("error");
});
