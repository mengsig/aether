import type { archon } from "@aether-ui/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { expect, test } from "vitest";
import { StageAttempt } from "./StageAttempt.tsx";
import { deadlineFor } from "./settings.tsx";

test("links a recorded agent to its conversation when its connection is known", () => {
  const stage: archon.StageRun = {
    id: "stage-1", run: "run-1", stage_id: "answer", attempt: 1, fan_index: 0,
    state: { state: "completed" }, choice: { model: "example/model", effort: "low" },
    cost_usd: 0, started_at: "2026-11-04T10:00:00Z", aether_agent: "agent-1",
  };
  const render = (conn: string | null) => renderToStaticMarkup(<StageAttempt stage={stage} now={Date.parse(stage.started_at)} conn={conn} />);
  expect(render("aether:one")).toContain('#/agents/aether%3Aone/agent-1');
  expect(render(null)).not.toContain("Open conversation");
});

test("calculates a running deadline only with a recorded limit and valid start", () => {
  const stage: archon.StageRun = {
    id: "stage-1", run: "run-1", stage_id: "answer", attempt: 1, fan_index: 0,
    state: { state: "running" }, choice: { model: "example/model", effort: "low" },
    cost_usd: 0, started_at: "2026-11-04T10:00:00Z",
  };
  expect(deadlineFor([{ key: "answer", timeout_minutes: 5 }], stage)).toBe("2026-11-04T10:05:00.000Z");
  expect(deadlineFor([], stage)).toBeNull();
  expect(deadlineFor([{ key: "answer", timeout_minutes: 5 }], { ...stage, started_at: "invalid" })).toBeNull();
  expect(deadlineFor([{ key: "answer", timeout_minutes: 5 }], { ...stage, state: { state: "completed" } })).toBeNull();
});

test("renders structured findings and safe Markdown without exposing raw JSON", () => {
  const stage: archon.StageRun = {
    id: "stage-1", run: "run-1", stage_id: "review", attempt: 1, fan_index: 0,
    state: { state: "completed" }, choice: { model: { provider: "codex", model: "example" }, effort: "low" },
    cost_usd: 0, started_at: "2026-11-04T10:00:00Z", verdict: "unknown",
    summary: JSON.stringify({ verdict: "pass", findings: [{ issue: "**Readable**", impact: "[unsafe](javascript:alert(1))" }] }),
  };
  const html = renderToStaticMarkup(<StageAttempt stage={stage} now={Date.parse(stage.started_at)} conn={null} />);
  expect(html).toContain("<table>");
  expect(html).toContain("<strong>Readable</strong>");
  expect(html).toContain("Verdict");
  expect(html).not.toContain("href=\"javascript:");
  expect(html).not.toContain("unknown");
  expect(html).not.toContain("{&quot;verdict");
});
