// Deterministic synthetic conversations: an agent and a transcript that mixes prompts, answers in
// Markdown, reasoning, tool calls with their results and attachments, in the daemon's own shapes.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../../fleet/invariant.ts";

export const FAUX_AGENT = "agt_01JFAUX0000000000000000001";
export const FAUX_PARENT = "agt_01JFAUX0000000000000000000";

export function fauxAgent(id = FAUX_AGENT, overrides: Partial<aether.Agent> = {}): aether.Agent {
  return {
    id, name: id === FAUX_PARENT ? "planner" : "builder", role: id === FAUX_PARENT ? "operator" : "archon-stage:adaptive/build",
    run: "run_01JFAUX0000000000000000001", model: { provider: "claude-code", model: "opus" },
    config: { effort: "high", cost_budget: 25, token_budget: 4_000_000 }, scope: {},
    status: { state: "running" }, created_at: "2026-11-04T08:58:00Z", updated_at: "2026-11-04T09:00:00Z",
    last_event_at: "2026-11-04T09:12:00Z",
    ...(id === FAUX_PARENT ? {} : { parent: FAUX_PARENT }),
    stats: {
      turns: 42, tool_calls: 117, context_tokens: 61_250,
      usage: { input_tokens: 1_204_311, output_tokens: 88_402, cache_read_tokens: 3_900_120, cache_write_tokens: 210_004, cost_usd: 7.4312, cost_complete: true, cost_basis: "provider_reported" },
    },
    ...overrides,
  };
}

export const usage = (cost: number): aether.Usage => ({
  input_tokens: 12_400, output_tokens: 900, cache_read_tokens: 40_000, cache_write_tokens: 0, cost_usd: cost, cost_complete: true, cost_basis: "provider_reported",
});

export const text = (role: aether.MessageRole, value: string): aether.Message => ({ role, content: [{ type: "text", text: value }] });

const ANSWERS = [
  "Read `crates/aether-daemon/src/projections.rs`: **transcript_page** pages by seq.\n\n- `after` pages forward\n- `before` pages back",
  "The gap fallback rereads from the last entry, so nothing is lost.",
  "Done. The tests pass:\n\n```sh\nnpm run check\n```",
];

/** One exchange of four messages; `i` picks which kind, so long transcripts mix every kind. */
function exchange(i: number): { message: aether.Message; invocation?: string }[] {
  const invocation = `inv_${String(i).padStart(8, "0")}`;
  const tool = `toolu_${String(i).padStart(8, "0")}`;
  const answer = ANSWERS[i % ANSWERS.length] ?? "";
  if (i % 3 === 0) {
    return [
      { message: text("user", `Step ${i}: check the transcript paging.`) },
      { message: { role: "assistant", content: [
        { type: "thinking", text: `Step ${i}: read the projection first.` },
        { type: "text", text: "Reading the projection." },
        { type: "tool_use", id: tool, name: "read_file", input: { path: "crates/aether-daemon/src/projections.rs" } },
      ] }, invocation },
      { message: { role: "user", content: [{ type: "tool_result", tool_use_id: tool, content: [{ type: "text", text: "pub fn transcript_page(…) { … }" }] }] } },
      { message: text("assistant", answer), invocation: `${invocation}b` },
    ];
  }
  const attachment: aether.ContentBlock = { type: "image_ref", id: `att_${i}`, media_type: "image/png", name: `screen-${i}.png`, size_bytes: 48_213 };
  return [
    { message: { role: "user", content: [{ type: "text", text: `Step ${i}: here is the screen.` }, attachment] } },
    { message: text("assistant", answer), invocation },
    { message: text("user", "Thanks, go on.") },
    { message: text("assistant", `Continuing with step ${i + 1}.`), invocation: `${invocation}b` },
  ];
}

/** `count` transcript messages, in the order a daemon would have committed them. */
export function fauxConversation(count: number): { message: aether.Message; invocation?: string }[] {
  invariant(Number.isInteger(count) && count >= 0, `message count ${count} is a whole number`);
  const messages: { message: aether.Message; invocation?: string }[] = [];
  for (let i = 0; messages.length < count; i += 1) messages.push(...exchange(i));
  return messages.slice(0, count);
}
