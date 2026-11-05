// The transcript fixture: `count` committed entries of one agent, as `get_transcript_page` returns
// them, mixing prompts, Markdown answers, reasoning, tool calls with results and attachments.

import type { aether } from "@aether-ui/protocol";
import { pad, seeded, stamp, type Random } from "./random.ts";

export const TRANSCRIPT_SIZE = 20_000;
export const TRANSCRIPT_AGENT = "agt_transcript";

const WORDS = ["the", "store", "reads", "a", "page", "of", "entries", "after", "its", "cursor", "and", "merges", "them", "by", "seq", "delta", "frame", "commit"];
const FILES = ["crates/aether-daemon/src/projections.rs", "web/packages/core/src/store.ts", "web/packages/aether/src/conversation/rows.ts"];

function sentence(random: Random, words: number): string {
  const out: string[] = [];
  for (let i = 0; i < words; i += 1) out.push(random.pick(WORDS));
  return `${out.join(" ")}.`;
}

/** A Markdown answer of a few paragraphs, with a list or a code block now and then. */
function answer(random: Random): string {
  const paragraphs = [sentence(random, 8 + random.below(30)), sentence(random, 8 + random.below(30))];
  if (random.below(3) === 0) paragraphs.push(`- \`${random.pick(FILES)}\`\n- ${sentence(random, 6)}`);
  if (random.below(4) === 0) paragraphs.push("```sh\nnpm run check\n```");
  return paragraphs.join("\n\n");
}

/** One message; a tool call's result follows it as the next entry, as a daemon commits them. */
function messages(random: Random, turn: number): { message: aether.Message; invocation: string | null }[] {
  const invocation = `inv_${pad(turn, 8)}`;
  const kind = random.below(4);
  if (kind === 0) return [{ message: { role: "user", content: [{ type: "text", text: sentence(random, 5 + random.below(20)) }] }, invocation: null }];
  if (kind === 1) {
    const tool = `toolu_${pad(turn, 8)}`;
    return [
      { message: { role: "assistant", content: [
        { type: "thinking", text: sentence(random, 10 + random.below(20)) },
        { type: "tool_use", id: tool, name: "read_file", input: { path: random.pick(FILES) } },
      ] }, invocation },
      { message: { role: "user", content: [{ type: "tool_result", tool_use_id: tool, content: [{ type: "text", text: sentence(random, 20 + random.below(60)) }] }] }, invocation: null },
    ];
  }
  if (kind === 2) {
    const image: aether.ContentBlock = { type: "image_ref", id: `att_${turn}`, media_type: "image/png", name: `screen-${turn}.png`, size_bytes: 40_000 + random.below(20_000) };
    return [{ message: { role: "user", content: [{ type: "text", text: "Here is the screen." }, image] }, invocation: null }];
  }
  return [{ message: { role: "assistant", content: [{ type: "text", text: answer(random) }] }, invocation }];
}

/** Entries with ascending seqs; seqs skip now and then, where other agents' events fall. */
export function transcriptEntries(seed: number, count = TRANSCRIPT_SIZE): aether.TranscriptEntry[] {
  if (!Number.isInteger(count) || count < 0) throw new Error(`transcript size ${count} is not a whole number`);
  const random = seeded(seed);
  const entries: aether.TranscriptEntry[] = [];
  let seq = 0;
  for (let turn = 0; entries.length < count; turn += 1) {
    for (const { message, invocation } of messages(random, turn)) {
      seq += 1 + random.below(3);
      entries.push({ seq, at: stamp(seq), invocation, message });
    }
  }
  entries.length = count;
  if (entries.some((entry, i) => i > 0 && entry.seq <= (entries[i - 1]?.seq ?? 0))) throw new Error("transcript seqs ascend");
  return entries;
}
