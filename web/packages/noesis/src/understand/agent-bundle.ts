// An Aether agent's bundle: who it is and where it stands, its instructions, and its last words.
// An agent's own output is never a check's verdict, so every item here is preliminary.

import type { aether } from "@aether-ui/protocol";
import { invariant, messageText } from "../learn/model.ts";
import { BRIEF_MAX_BYTES, bundleOf, evidence } from "./bundle.ts";

/** How many of the agent's newest messages a bundle quotes. */
export const AGENT_MESSAGES = 3;

function statusWords(status: aether.AgentStatus): string {
  switch (status.state) {
    case "completed": return status.summary ? `completed: ${status.summary}` : "completed";
    case "failed": return `failed: ${status.error}`;
    case "cancelled": return status.reason ? `cancelled: ${status.reason}` : "cancelled";
    case "imported": return `imported (was ${status.was})`;
    default: return status.state;
  }
}

/** The agent's newest prose, newest last: assistant text only, no tool calls. */
export function lastWords(entries: readonly aether.TranscriptEntry[], count = AGENT_MESSAGES): string[] {
  invariant(Number.isInteger(count) && count > 0, "a bundle quotes at least one message");
  const said = entries.filter((entry) => entry.message.role === "assistant").map((entry) => messageText(entry.message)).filter((text) => text !== "");
  const picked = said.slice(-count);
  invariant(picked.length <= count, "no more messages than asked for");
  return picked;
}

export function agentBundle(agent: aether.Agent, entries: readonly aether.TranscriptEntry[]): aether.EvidenceBundle {
  invariant(agent.id.length > 0 && agent.name.trim().length > 0, "an agent bundle reads a whole agent");
  invariant(entries.every((entry, index) => index === 0 || (entries[index - 1]?.seq ?? 0) < entry.seq), "transcript entries are in order");
  const ref = { kind: "agent", id: agent.id } as const;
  const identity = [
    `Agent ${agent.name}${agent.role ? ` (role ${agent.role})` : ""}`,
    `Status: ${statusWords(agent.status)}`,
    `Model: ${agent.model.provider}/${agent.model.model}`,
  ].join("\n");
  const items: aether.EvidenceItem[] = [evidence(ref, "preliminary", identity)];
  const instructions = agent.config.instructions?.trim();
  if (instructions) items.push(evidence(ref, "preliminary", `Instructions:\n${instructions}`, BRIEF_MAX_BYTES));
  for (const text of lastWords(entries)) items.push(evidence(ref, "preliminary", `The agent said:\n${text}`));
  return bundleOf(agent.name, items);
}
