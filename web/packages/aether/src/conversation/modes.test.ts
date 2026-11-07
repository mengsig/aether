// The desktop's view-mode rules against the TUI's: the same parity fixture, row for row, and the
// web transcript's own parts classified into that fixture's row kinds.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { splitInbox } from "./inbox.ts";
import { applies, initialMode, outcome, ROW_KINDS, type ModeRules, type RowKind } from "./modes.ts";
import { entryRows, pairTools, type ConversationRow } from "./rows.ts";
import { partKind, taskRowKey, viewRows } from "./view.ts";

interface Fixture {
  readonly version: number;
  readonly modes: Readonly<Record<string, (ModeRules & { scope?: aether.ViewModeScope }) | null>>;
  readonly rows: readonly { readonly id: string; readonly kind: RowKind }[];
  readonly expect: Readonly<Record<string, Readonly<Record<string, string>>>>;
  readonly scope: readonly { readonly mode: string; readonly role: string | null; readonly name: string | null; readonly applies: boolean }[];
}

// Read from the repository at test time, never bundled: the TUI's own test reads the same file.
function readFixture(): Fixture {
  const node = (globalThis as unknown as { process: { getBuiltinModule(id: "node:fs"): { readFileSync(path: string, encoding: "utf8"): string } } }).process;
  const dir = (import.meta as ImportMeta & { dirname: string }).dirname;
  const file = `${dir}/../../../../../crates/aether-tui/tests/fixtures/view_mode_parity.json`;
  return JSON.parse(node.getBuiltinModule("node:fs").readFileSync(file, "utf8")) as Fixture;
}
const fixture = readFixture();
const kindOutcome = (mode: string, kind: RowKind): string => {
  const row = fixture.rows.find((r) => r.kind === kind);
  if (row === undefined) throw new Error(`the fixture has no ${kind} row`);
  return fixture.expect[mode]?.[row.id] ?? "missing";
};

describe("the shared parity fixture", () => {
  test("names every row kind the desktop knows, once", () => {
    expect(fixture.version).toBe(1);
    expect(fixture.rows.map((r) => r.kind).sort()).toEqual([...ROW_KINDS].sort());
  });

  test.each(Object.keys(fixture.expect))("%s draws each row as the TUI does", (mode) => {
    const rules = fixture.modes[mode] ?? null;
    const drawn = Object.fromEntries(fixture.rows.map((row) => [row.id, outcome(rules, row.kind)]));
    expect(drawn).toEqual(fixture.expect[mode]);
  });

  test("scopes admit the agents the TUI admits", () => {
    for (const c of fixture.scope) {
      const mode = fixture.modes[c.mode];
      const admitted = mode === null || mode === undefined ? true : applies(mode, { name: c.name ?? "", role: c.role ?? "" });
      expect(admitted, `${c.mode} for ${c.role}/${c.name}`).toBe(c.applies);
    }
  });
});

const ENTRY_AT = "2026-11-07T09:00:00Z";
const user = (text: string): aether.Message => ({ role: "user", content: [{ type: "text", text }] });
const call = (id: string, name: string, input: unknown): aether.ContentBlock => ({ type: "tool_use", id, name, input });
const result = (id: string, text: string, isError = false): aether.ContentBlock => ({ type: "tool_result", tool_use_id: id, content: [{ type: "text", text }], is_error: isError });

/** A delegated agent's transcript holding one part of each kind its durable messages can carry. */
function transcript(): ConversationRow[] {
  const messages: aether.Message[] = [
    user("Build the chat view modes."),
    user("Please also keep the draft."),
    { role: "assistant", content: [{ type: "thinking", text: "plan" }, { type: "text", text: "On it." },
      call("t_ok", "Bash", { command: "ls" }), call("t_fail", "bash", { command: "false" }), call("t_edit", "edit", { path: "a" }),
      call("t_denied", "write_file", { path: "b" }), call("t_cancel", "web_fetch", { url: "u" }), call("t_run", "grep", { pattern: "x" }),
      call("t_mem", "remember", { text: "fact" }), call("t_spawn", "mcp__aether__delegate", { name: "worker", objective: "verify" }),
      call("t_wait", "wait_for", { tasks: ["tsk_1"] }), call("t_send", "send_message", { to: "agt_peer", content: "hi" }),
      call("t_ask", "ask_user", { question: "Which?" })] },
    { role: "user", content: [result("t_ok", "a.txt"), result("t_fail", "exit 1", true), result("t_edit", "no match", true),
      result("t_denied", "Tool call denied: by rule", true), result("t_cancel", "Tool call was cancelled.", true),
      result("t_mem", "saved"), result("t_spawn", "agt_child started"), result("t_wait", "done"), result("t_send", "sent"), result("t_ask", "answer")] },
    user("[inbox 4] Message from agt_peer:\nready\n[inbox 5] Task tsk_1 completed: shipped\n[inbox 6] Child agent agt_child failed: boom\n[inbox 7] Approval apr_1 approved by user\n[inbox 8] Woken: timer"),
    { role: "assistant", content: [{ type: "compaction", content: "earlier" }] },
  ];
  const entries = messages.map((message, i): aether.TranscriptEntry => ({ seq: i + 1, at: ENTRY_AT, invocation: message.role === "assistant" ? `inv_${i}` : null, message }));
  return entryRows(entries, pairTools(entries), new Map());
}

describe("the desktop transcript's row kinds", () => {
  const rows = transcript();
  const task = taskRowKey(rows, true);
  const kinds = rows.flatMap((row) => row.parts.map((part) => partKind(part, row, row.key === task)));

  test("each durable part takes the kind the TUI gives the same event", () => {
    expect(kinds).toEqual([
      "user_task", "user_operator", "reasoning", "assistant_text",
      "tool_succeeded", "tool_failed", "edit_failed", "tool_denied", "tool_cancelled", "tool_running",
      "memory", "spawned", "waiting", "sent", "question",
      "user_message", "child_finished", "child_failed", "approval", "notice_status", "compaction",
    ]);
  });

  test("only rows a transcript does not carry are left to the TUI's event feed", () => {
    // The agent header and live call or session notices are drawn outside the transcript list.
    const missing = ROW_KINDS.filter((k) => !kinds.includes(k) && k !== "usage" && k !== "error");
    expect(missing).toEqual(["header", "tool_awaiting", "notice_error", "compaction_failed"]);
  });

  test.each(Object.keys(fixture.expect))("%s keeps exactly the parts its rules draw", (mode) => {
    const rules = fixture.modes[mode] ?? null;
    const views = viewRows(rows, { rules, taskRow: task }, new WeakMap());
    const drawn = views.flatMap((v) => v.parts.map((p) => `${p.kind}:${p.outcome}`));
    const wanted = kinds.filter((k) => kindOutcome(mode, k) !== "hidden").map((k) => `${k}:${kindOutcome(mode, k)}`);
    expect(drawn).toEqual(wanted);
  });

  test("a root agent's first message is the operator's, not a task", () => {
    expect(taskRowKey(rows, false)).toBeNull();
  });
});

describe("inbox turns", () => {
  test("split into deliveries, keeping what a person typed as their own text", () => {
    expect(splitInbox("[inbox 1] Message from user:local_socket#7: [authenticated operator instruction]\nship it\n[inbox 2] Task tsk_9 was cancelled")).toEqual([
      { kind: "operator", text: "ship it" },
      { kind: "failed", subject: "tsk_9", text: "" },
    ]);
  });

  test("leave a plain message whole", () => {
    expect(splitInbox("  hello\nthere ")).toEqual([{ kind: "operator", text: "hello\nthere" }]);
  });
});

describe("the mode an agent opens in", () => {
  const mode = (name: string, extra: Partial<aether.ViewMode> = {}): aether.ViewMode => ({ name, builtin: false, density: "compact", show: ["user_text"], ...extra });
  const catalog = [mode("standard", { builtin: true }), mode("focus", { builtin: true }), mode("brief"), mode("calm", { scope: { role: "archon-orchestrator" } })];
  const orchestrator = { name: "Archon", role: "archon-orchestrator" };
  const worker = { name: "w", role: "worker" };

  test("is a user mode scoped to the agent, else standard", () => {
    expect(initialMode(catalog, orchestrator, null)).toBe("calm");
    expect(initialMode(catalog, worker, null)).toBe("standard");
  });

  test("is the saved choice while that mode still applies", () => {
    expect(initialMode(catalog, orchestrator, "focus")).toBe("focus");
    expect(initialMode(catalog, worker, "calm"), "a scope mismatch falls back").toBe("standard");
    expect(initialMode(catalog, worker, "gone"), "a removed mode falls back").toBe("standard");
  });
});
