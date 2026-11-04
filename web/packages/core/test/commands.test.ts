import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import {
  AETHER_DEVICE_SELF, AETHER_HOST_ONLY, AETHER_KEYED, AETHER_OBSERVE, AETHER_OPERATE, aetherAccess, archonAccess, checkAllowed, withIdempotencyKey,
} from "../src/commands.ts";

const repo = path.join(import.meta.dirname, "..", "..", "..", "..");
const generated = readFileSync(path.join(repo, "web", "packages", "protocol", "src", "aether.ts"), "utf8");
const accessRs = readFileSync(path.join(repo, "crates", "aether-protocol", "src", "access.rs"), "utf8");

/** Every generated command interface: its tag and whether it takes an idempotency key. */
function generatedCommands(): Map<string, boolean> {
  const commands = new Map<string, boolean>();
  for (const match of generated.matchAll(/export interface \w+Command \{\n([\s\S]*?)\n\}/g)) {
    const body = match[1] ?? "";
    const tag = /"cmd": "(\w+)";/.exec(body)?.[1];
    if (tag) commands.set(tag, body.includes('"idempotency_key"'));
  }
  expect(commands.size).toBeGreaterThan(100);
  return commands;
}

const snake = (name: string) => name.replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase();
const CLASS: Record<string, string> = { Observe: "observe", Operate: "operate", HostOnly: "host_only", RemoteAdmin: "host_only", DeviceSelf: "device_self" };

/** The unguarded arms of `Command::access`: command tag to class. */
function rustClasses(): Map<string, string> {
  const classes = new Map<string, string>();
  const lines = accessRs.slice(accessRs.indexOf("match self {"), accessRs.indexOf("#[cfg(test)]")).split("\n");
  let pending: string[] = [];
  for (const [index, line] of lines.entries()) {
    pending.push(...[...line.matchAll(/Self::(\w+)/g)].map((m) => snake(m[1] ?? "")));
    const arrow = line.indexOf("=>");
    if (arrow < 0) continue;
    const after = line.slice(arrow + 2).trim();
    const target = after === "{" ? (lines[index + 1] ?? "").trim() : after;
    const access = /^Access::(\w+)/.exec(target)?.[1];
    if (access && !line.includes(" if ")) for (const name of pending) classes.set(name, CLASS[access] ?? "?");
    pending = [];
  }
  expect(classes.size).toBeGreaterThan(90);
  return classes;
}

describe("command access", () => {
  it("classifies every generated Aether command exactly once", () => {
    const sets = [AETHER_OBSERVE, AETHER_OPERATE, AETHER_HOST_ONLY, AETHER_DEVICE_SELF];
    for (const cmd of generatedCommands().keys()) {
      expect(sets.filter((set) => set.has(cmd)).length, cmd).toBe(1);
    }
  });

  it("agrees with access.rs for every unguarded arm", () => {
    for (const [cmd, access] of rustClasses()) {
      expect(aetherAccess({ cmd }), cmd).toBe(access);
    }
  });

  it("keys exactly the commands whose schema takes an idempotency key", () => {
    const keyed = [...generatedCommands()].filter(([, takesKey]) => takesKey).map(([cmd]) => cmd).sort();
    expect([...AETHER_KEYED].sort()).toEqual(keyed);
  });

  it("widens access where a field reaches further", () => {
    expect(aetherAccess({ cmd: "create_memory", spec: { proposed: true } })).toBe("host_only");
    expect(aetherAccess({ cmd: "create_memory", spec: { proposed: false } })).toBe("operate");
    expect(aetherAccess({ cmd: "memories", query: { proposed: true } })).toBe("host_only");
    expect(aetherAccess({ cmd: "list_devices", include_revoked: true })).toBe("host_only");
    expect(aetherAccess({ cmd: "list_devices" })).toBe("operate");
    expect(aetherAccess({ cmd: "mcp_servers", refresh: true })).toBe("operate");
    expect(aetherAccess({ cmd: "mcp_servers" })).toBe("observe");
    expect(aetherAccess({ cmd: "update_agent", patch: { tools: [] } })).toBe("host_only");
    expect(aetherAccess({ cmd: "update_agent", patch: { name: "x" } })).toBe("operate");
    expect(aetherAccess({ cmd: "add_approval_rule", scope: { scope: "everywhere" } })).toBe("host_only");
    expect(aetherAccess({ cmd: "resolve_approval", remember: { scope: { scope: "agent" } } })).toBe("operate");
    expect(aetherAccess({ cmd: "resolve_submission", command: { cmd: "get_agent" } })).toBe("operate");
    expect(aetherAccess({ cmd: "resolve_submission", command: { cmd: "shutdown" } })).toBe("host_only");
    expect(aetherAccess({ cmd: "not_a_command_yet" })).toBe("operate");
    expect(aetherAccess({ cmd: "noesis_configure", patch: { engineering_context: true } })).toBe("operate");
    expect(aetherAccess({ cmd: "noesis_configure", patch: { enabled: false } })).toBe("host_only");
    expect(aetherAccess({ cmd: "noesis_configure", patch: { budget: { background_tokens_daily: 9 } } })).toBe("host_only");
    expect(aetherAccess({ cmd: "noesis_configure", patch: { budget: {} } })).toBe("operate");
    expect(aetherAccess({ cmd: "noesis_configure", patch: { billing: { allow_api: true } } })).toBe("host_only");
    expect(aetherAccess({ cmd: "noesis_configure", patch: { prompts: { overrides: false } } })).toBe("host_only");
  });

  it("keeps archond's host commands and global hard pause off devices", () => {
    expect(archonAccess({ cmd: "task_graph" })).toBe("observe");
    expect(archonAccess({ cmd: "merge_item" })).toBe("host_only");
    expect(archonAccess({ cmd: "pause", scope: "global", hard: true })).toBe("host_only");
    expect(archonAccess({ cmd: "pause", scope: "global" })).toBe("operate");
  });

  it("refuses by standing", () => {
    expect(() => checkAllowed("operate", "observer")).toThrow(/Observer/);
    expect(() => checkAllowed("host_only", "operator")).toThrow(/own host/);
    expect(() => checkAllowed("observe", undefined)).toThrow(/not ready/);
    expect(() => checkAllowed("device_self", "observer")).not.toThrow();
    expect(() => checkAllowed("host_only", "local")).not.toThrow();
  });

  it("adds one idempotency key to keyed commands and keeps a caller's own", () => {
    expect(withIdempotencyKey({ cmd: "send_message" }, () => "k1")).toEqual({ cmd: "send_message", idempotency_key: "k1" });
    expect(withIdempotencyKey({ cmd: "send_message", idempotency_key: "mine" }, () => "k1").idempotency_key).toBe("mine");
    expect(withIdempotencyKey({ cmd: "get_agent" }, () => "k1")).toEqual({ cmd: "get_agent" });
  });
});
