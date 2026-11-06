import { describe, expect, test } from "vitest";
import type { aether } from "@aether-ui/protocol";
import { defaultModel, defaultWorkspace } from "./data.ts";

const workspace = (path: string, exists: boolean, active_at: string): aether.Workspace => ({ path, exists, active_at, agents: 0, running: 0 });
const model = (name: string, configured = true, is_default = false): aether.ModelInfo => ({ name, configured, is_default,
  model: { provider: "test", model: name }, provider_name: "Test", capabilities: { context_window: 128000, max_output_tokens: 8192 } });

describe("start defaults", () => {
  test("uses the most recent existing workspace before home completions", () => {
    const home = { entries: [{ path: "/home/user/repo", is_dir: true }], truncated: false };
    expect(defaultWorkspace([workspace("/old", true, "2026-01-01"), workspace("/gone", false, "2026-03-01"), workspace("/recent", true, "2026-02-01")], home)).toBe("/recent");
    expect(defaultWorkspace([workspace("/gone", false, "2026-03-01")], home)).toBe("/home/user/repo");
  });
  test("ignores files in home completions and stays empty when no directory exists", () => {
    expect(defaultWorkspace([], { entries: [{ path: "/home/user/file", is_dir: false }, { path: "/home/user/dir", is_dir: true }], truncated: false })).toBe("/home/user/dir");
    expect(defaultWorkspace([])).toBe("");
    expect(defaultWorkspace([], { entries: [], truncated: false })).toBe("");
  });
  test("prefers the configured default and otherwise the first ready model", () => {
    expect(defaultModel([model("first"), model("default", true, true)])).toBe("test/default");
    expect(defaultModel([model("offline", false, true), model("ready")])).toBe("test/ready");
    expect(defaultModel([model("offline", false)])).toBe("");
    expect(defaultModel([])).toBe("");
  });
});
