// The transcript's small pure helpers: tool row summaries, the TeX fallback, find, and who speaks.

import type { aether } from "@aether-ui/protocol";
import { describe, expect, test } from "vitest";
import { prettyTex } from "./math.ts";
import { speakerOf, toolActivity, type ConversationRow, type Part } from "./rows.ts";
import { foldCase, foldsToOpen, locate, searchRows, stepMatch } from "./search.ts";
import { argumentSummary, toolGlyph } from "./tool-summary.ts";

describe("a tool row's summary", () => {
  test.each([
    ["Bash", "$"], ["Edit", "✎"], ["Grep", "⌕"], ["Read", "≡"], ["ls", "≡"], ["WebFetch", "↗"], ["Task", "⇢"], ["tools", "•"], ["mystery", "•"],
  ])("gives %s the glyph %s", (name, glyph) => expect(toolGlyph(name)).toBe(glyph));

  test("prefers the argument a call is known by", () => {
    expect(argumentSummary(JSON.stringify({ description: "List", command: "ls -la" }))).toBe("ls -la");
    expect(argumentSummary(JSON.stringify({ file_path: "/src/a.ts", limit: 20 }))).toBe("/src/a.ts");
    expect(argumentSummary(JSON.stringify({ limit: 20, deep: true, nested: { a: 1 } }))).toBe("limit: 20, deep: true");
    expect(argumentSummary("{}")).toBe("");
  });

  test("reads a key argument out of JSON still being streamed", () => {
    expect(argumentSummary('{"command":"git status\\n')).toBe("git status");
    expect(argumentSummary('{"other":')).toBe("");
  });

  test("keeps to one line of bounded length", () => {
    const summary = argumentSummary(JSON.stringify({ command: `echo ${"x".repeat(400)}\nnext` }));
    expect(summary).not.toContain("\n");
    expect(summary).toHaveLength(140);
    expect(summary.endsWith("…")).toBe(true);
  });
});

describe("the TeX fallback", () => {
  test.each([
    ["x^2 + y^2 = z^2", "x² + y² = z²"],
    ["\\alpha \\cdot \\beta", "α · β"],
    ["\\frac{a}{b}", "a⁄b"],
    ["\\frac{1}{2+3}", "1⁄(2+3)"],
    ["\\frac{x^2}{\\pi}", "(x²)⁄π"],
    ["\\sqrt{x}", "√(x)"],
    ["a_{ij} \\leq \\infty", "aᵢⱼ ≤ ∞"],
    ["\\text{cost} = \\sum_i c_i", "cost = ∑ᵢ cᵢ"],
    ["\\left( x \\right)", "( x )"],
    ["e^{i\\pi}", "e^(iπ)"],
  ])("shows %s as %s", (tex, shown) => expect(prettyTex(tex)).toBe(shown));

  test("leaves unknown commands and very long input as written", () => {
    expect(prettyTex("\\unknown{x}")).toBe("\\unknownx");
    const long = "\\alpha".repeat(1_000);
    expect(prettyTex(long)).toBe(long);
  });
});

const row = (key: string, parts: readonly Part[]): ConversationRow => ({
  key, role: "assistant", speaker: { kind: "assistant", from: null }, seq: 1, at: null, invocation: null, parts, live: null, meta: null,
});

describe("find", () => {
  const rows = [
    row("e:1", [{ kind: "text", text: "Needle and needle" }]),
    row("e:2", [{ kind: "text", text: "nothing" }]),
    row("e:3", [{ kind: "reasoning", text: "a needle in thought", redacted: false }, { kind: "tool", id: "toolu_1", name: "grep", input: "{}", result: { text: "needle", isError: false, images: 0 } }]),
  ];

  test("counts matches without case, per row, and locates the n-th", () => {
    const result = searchRows(rows, " NEEDLE ");
    expect(result).toMatchObject({ query: "needle", rows: [0, 2], ends: [2, 4], total: 4 });
    expect([0, 1, 2, 3].map((n) => locate(result, n))).toEqual([{ row: 0, nth: 0 }, { row: 0, nth: 1 }, { row: 2, nth: 0 }, { row: 2, nth: 1 }]);
    expect(searchRows(rows, "  ").total).toBe(0);
  });

  test("folds case one character at a time, so a letter that grows when lowered still maps back", () => {
    const { folded, origin } = foldCase("İstanbul");
    expect(folded.length).toBe(9);
    expect(origin).toEqual([0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
    expect(searchRows([row("e:9", [{ kind: "text", text: "İstanbul" }])], "L").total).toBe(1);
  });

  test("names the folded parts to open for a match", () => {
    expect(foldsToOpen(rows[2] ?? row("x", []), "needle")).toEqual(["f:e:3:0", "t:toolu_1"]);
    expect(foldsToOpen(rows[0] ?? row("x", []), "needle")).toEqual([]);
  });

  test("steps wrap at both ends", () => {
    expect(stepMatch(3, 1, 4)).toBe(0);
    expect(stepMatch(0, -1, 4)).toBe(3);
    expect(stepMatch(-1, 1, 4)).toBe(0);
  });
});

describe("who speaks in a user turn", () => {
  const said = (value: string): Part[] => [{ kind: "text", text: value }];

  test("an operator's prompt, an agent's message, an inbox notice", () => {
    expect(speakerOf("user", said("Do it"))).toEqual({ speaker: { kind: "user", from: null }, parts: said("Do it") });
    expect(speakerOf("user", said("[inbox 4] Message from agt_x:\nHello"))).toEqual({ speaker: { kind: "agent", from: "agt_x" }, parts: said("Hello") });
    expect(speakerOf("user", said("[inbox 5] Message from user:device: [authenticated operator instruction]\nStop"))).toEqual({ speaker: { kind: "user", from: "user:device" }, parts: said("Stop") });
    expect(speakerOf("user", said("[inbox 6] Task tsk_1 completed")).speaker.kind).toBe("notice");
  });

  test("a turn holding several inbox items keeps every header", () => {
    const text = "[inbox 1] Message from agt_a:\nOne\n[inbox 2] Message from agt_b:\nTwo";
    expect(speakerOf("user", said(text))).toEqual({ speaker: { kind: "agent", from: "agt_a" }, parts: said(text) });
  });
});

describe("tool activity", () => {
  const event = (seq: number, kind: unknown, agent = "agt_1") => ({ seq, id: `evt_${seq}`, at: "", actor: { actor: "system" }, agent_id: agent, kind }) as unknown as aether.Event;

  test("follows each call by its provider id to its outcome and duration", () => {
    const events = [
      event(1, { type: "tool.requested", call: { id: "call_1", provider_call_id: "toolu_1" } }),
      event(2, { type: "tool.requested", call: { id: "call_2", provider_call_id: "toolu_2" } }),
      event(3, { type: "tool.started", call: "call_1" }),
      event(4, { type: "tool.started", call: "call_2" }),
      event(5, { type: "tool.succeeded", call: "call_1", outcome: { duration_ms: 120, is_error: false } }),
      event(6, { type: "tool.requested", call: { id: "call_3", provider_call_id: "toolu_3" } }, "agt_other"),
    ];
    expect([...toolActivity(events, "agt_1")]).toEqual([
      ["toolu_1", { state: "ok", durationMs: 120 }],
      ["toolu_2", { state: "running", durationMs: null }],
    ]);
    const failed = toolActivity([...events, event(7, { type: "tool.failed", call: "call_2", duration_ms: 9 })], "agt_1");
    expect(failed.get("toolu_2")).toEqual({ state: "error", durationMs: 9 });
  });
});
