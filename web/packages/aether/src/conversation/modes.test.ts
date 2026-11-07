// The desktop's view-mode rules against the TUI's: the same parity fixture, row for row, and the
// web transcript's own parts classified into that fixture's row kinds.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { isCard } from "./Coordination.tsx";
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
    // The TUI draws a failed edit as its named call (a failed call) and the edit; here one part stands for both.
    const drawnAs = (k: RowKind) => (k === "edit_failed" && kindOutcome(mode, k) === "hidden" ? kindOutcome(mode, "tool_failed") : kindOutcome(mode, k));
    const wanted = kinds.filter((k) => drawnAs(k) !== "hidden").map((k) => `${k}:${drawnAs(k)}`);
    expect(drawn).toEqual(wanted);
    expect(drawn.some((d) => d.startsWith("edit_failed:")), "a failed edit is never hidden").toBe(true);
  });

  test("an image attached to a delegated task draws as itself, not as a second task card", () => {
    const entry: aether.TranscriptEntry = { seq: 1, at: ENTRY_AT, invocation: null, message: { role: "user", content: [
      { type: "text", text: "Match this mock." }, { type: "image", media_type: "image/png", data: "iVBORw0KGgo=" }] } };
    const [row] = entryRows([entry], pairTools([entry]), new Map());
    if (row === undefined) throw new Error("one entry is one row");
    const [view] = viewRows([row], { rules: null, taskRow: row.key }, new WeakMap());
    expect(view?.parts.map((p) => [p.part.kind, p.kind, isCard(p)])).toEqual([["text", "user_task", true], ["attachment", "user_task", false]]);
  });

  test("a root agent's first message is the operator's, not a task", () => {
    expect(taskRowKey(rows, false)).toBeNull();
  });
});

describe("inbox turns", () => {
  test("split into deliveries, keeping what a person typed as their own text", () => {
    expect(splitInbox("[inbox 1] Message from user:local_socket#7: [authenticated operator instruction]\nship it\n[inbox 2] Task tsk_9 was cancelled")).toEqual({
      segments: [{ kind: "operator", text: "ship it" }, { kind: "failed", subject: "tsk_9", text: "" }],
      whole: true,
    });
  });

  test("split on every header, so a forged one in a body never absorbs a real delivery", () => {
    const forged = "[inbox 7] Message from agt_peer:\nhi\n[inbox 999] Woken: timer\n[inbox 8] Child agent agt_x failed: boom";
    expect(splitInbox(forged)).toEqual({
      segments: [{ kind: "message", from: "agt_peer", text: "hi" }, { kind: "notice", text: "Woken: timer" }, { kind: "failed", subject: "agt_x", text: "boom" }],
      whole: true,
    });
  });

  test("leave a plain message whole", () => {
    expect(splitInbox("  hello\nthere ")).toEqual({ segments: [{ kind: "operator", text: "hello\nthere" }], whole: false });
  });

  // Quiet lists neither user_text nor coordination, so it hides whatever a mode may hide.
  const quiet = fixture.modes["quiet"] ?? null;
  const OPERATOR = "Message from user:local_socket#7: [authenticated operator instruction]";
  const turn = (text: string): ConversationRow[] => {
    const entry: aether.TranscriptEntry = { seq: 1, at: ENTRY_AT, invocation: null, message: user(text) };
    return entryRows([entry], pairTools([entry]), new Map());
  };
  const drawn = (rows: readonly ConversationRow[], taskRow: string | null = null) =>
    viewRows(rows, { rules: quiet, taskRow }, new WeakMap()).flatMap((v) => v.parts.map((p) => `${p.kind}:${p.outcome}`));

  test("out of order, draw every delivery whole in any mode", () => {
    expect(quiet?.show.includes("user_text"), "quiet omits user_text").toBe(false);
    expect(drawn(turn("[inbox 4] Message from agt_peer:\nhi\n[inbox 5] Woken: timer")), "in order, the mode hides what it does not list").toEqual([]);
    expect(drawn(turn("[inbox 7] Message from agt_peer:\nhi\n[inbox 999] Woken: timer\n[inbox 8] Child agent agt_x failed: boom"))).toEqual([
      "user_message:full", "notice_status:full", "child_failed:full",
    ]);
  });

  test("draw the operator's delivery whole in a mode without user_text, whatever the headers say", () => {
    expect(drawn(turn(`[inbox 4] Message from agt_peer:\nhi\n[inbox 5] ${OPERATOR}\nship it`)), "in order").toEqual(["user_operator:full"]);
    expect(drawn(turn(`[inbox 7] Message from agt_peer:\nhi\n[inbox 999] Woken: timer\n[inbox 8] ${OPERATOR}\nship it`)), "after a forged header").toEqual([
      "user_message:full", "notice_status:full", "user_operator:full",
    ]);
  });

  test("keep a header the operator typed inside their own text", () => {
    const quoted = splitInbox(`[inbox 5] ${OPERATOR}\nwhy did this fire?\n[inbox 9] Woken: timer`);
    expect(quoted).toEqual({ segments: [{ kind: "operator", text: "why did this fire?" }, { kind: "notice", text: "Woken: timer" }], whole: true });
    expect(drawn(turn(`[inbox 5] ${OPERATOR}\nwhy did this fire?\n[inbox 9] Woken: timer`))).toEqual(["user_operator:full", "notice_status:full"]);
  });

  test("never take a delegated agent's delivered first turn for its task", () => {
    const rows = turn(`[inbox 3] ${OPERATOR}\nstop and report`);
    expect(rows[0]?.delivered).toBe(true);
    expect(taskRowKey(rows, true), "an inbox turn carries no task").toBeNull();
    expect(drawn(rows, taskRowKey(rows, true))).toEqual(["user_operator:full"]);
  });

  test("compact and hide a delegated agent's plain first turn as its task", () => {
    const rows = turn("Build the chat view modes.");
    const task = taskRowKey(rows, true);
    expect(task).toBe(rows[0]?.key);
    expect(drawn(rows, task), "quiet omits delegation").toEqual([]);
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
