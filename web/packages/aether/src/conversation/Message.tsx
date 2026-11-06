// One transcript row: who speaks (the operator on a tonal surface with a mark, the agent plain on the
// page, a notice quietly), then its text, folded reasoning, tool rows, attachments, model and cost.

import { Badge } from "@aether-ui/ui";
import { createContext, memo, useContext, useId, type ReactNode } from "react";
import { invariant } from "../fleet/invariant.ts";
import { formatBytes, formatCost, formatDuration, formatTime, formatTokens } from "./format.ts";
import { Markdown } from "./markdown.tsx";
import type { ConversationRow, InvocationMeta, Part, Speaker } from "./rows.ts";
import { foldKey, orphanKey, toolKey, useToggle } from "./toggles.ts";
import { OrphanResult, ToolCall } from "./ToolCall.tsx";

/** Reasoning or a compaction summary: one quiet line that opens to the text. */
function Fold({ label, text, toggleKey }: { readonly label: string; readonly text: string | null; readonly toggleKey: string }) {
  invariant(toggleKey.startsWith("f:"), `fold toggle ${toggleKey} is a fold key`);
  const [open, flip] = useToggle(toggleKey);
  const panel = useId();
  if (text === null) return <p className="conv-fold conv-note">{label}</p>;
  return (
    <div className="conv-fold">
      <button type="button" className="conv-fold-toggle" aria-expanded={open} aria-controls={panel} onClick={flip}>
        <span className="conv-chevron" aria-hidden="true" />{label}
      </button>
      <p className="conv-fold-text" id={panel} hidden={!open}>{text}</p>
    </div>
  );
}

function PartView({ part, live, rowKey, index }: { readonly part: Part; readonly live: boolean; readonly rowKey: string; readonly index: number }): ReactNode {
  switch (part.kind) {
    case "text": return <Markdown text={part.text} />;
    case "reasoning":
      return <div className="conv-reasoning"><Fold label={part.redacted ? "Thinking (redacted)" : "Thinking"} text={part.redacted ? null : part.text} toggleKey={foldKey(rowKey, index)} /></div>;
    case "tool": return <ToolCall part={part} live={live} toggleKey={toolKey(rowKey, index, part.id)} />;
    case "orphan-result": return <OrphanResult part={part} toggleKey={orphanKey(rowKey, part.toolUseId)} />;
    case "attachment":
      return (
        <span className="conv-attachment">
          <span className="conv-attachment-name">{part.name}</span>
          <span className="conv-note">{part.mediaType}{part.sizeBytes === null ? "" : ` · ${formatBytes(part.sizeBytes)}`}</span>
        </span>
      );
    case "compaction":
      return <div className="conv-reasoning conv-compaction"><Fold label="Earlier conversation compacted" text={part.text} toggleKey={foldKey(rowKey, index)} /></div>;
  }
}

function MetaLine({ meta }: { readonly meta: InvocationMeta }) {
  const bits: string[] = [];
  if (meta.model) bits.push(meta.model);
  if (meta.usage) bits.push(formatCost(meta.usage), `${formatTokens(meta.usage.input_tokens)} in · ${formatTokens(meta.usage.output_tokens)} out`);
  if (meta.durationMs !== null) bits.push(formatDuration(meta.durationMs));
  return (
    <span className="conv-meta">
      {bits.join(" · ")}
      {meta.error === null ? null : <Badge tone="danger">Failed: {meta.error}</Badge>}
    </span>
  );
}

function LiveBadges({ row }: { readonly row: ConversationRow }) {
  if (!row.live) return null;
  if (row.live.recovering) return <Badge tone="warning">Catching up</Badge>;
  return (
    <>
      <Badge tone="info">Streaming</Badge>
      {row.live.partial ? <Badge tone="warning">Joined mid-message</Badge> : null}
    </>
  );
}

/** The agent's own name, for the head of its turns. */
export const AgentNameContext = createContext("Agent");

/** The name a row's head shows, and the initial its mark carries. */
export function speakerName(speaker: Speaker, agentName: string): { name: string; mark: string } {
  switch (speaker.kind) {
    case "assistant": return { name: agentName, mark: (agentName[0] ?? "A").toUpperCase() };
    case "notice": return { name: "Notice", mark: "i" };
    case "agent": return { name: speaker.from ?? "Another agent", mark: "⇢" };
    case "user": {
      const named = speaker.from?.split(":")[1];
      const name = named === undefined || named === "" || /^(local_socket|tcp|device)/.test(named) ? "You" : named;
      return { name, mark: (name[0] ?? "Y").toUpperCase() };
    }
  }
}

function MessageHead({ row }: { readonly row: ConversationRow }) {
  const { name, mark } = speakerName(row.speaker, useContext(AgentNameContext));
  const marked = row.speaker.kind === "user" || row.speaker.kind === "agent";
  return (
    <header className="conv-msg-head">
      {marked ? <span className="conv-mark" aria-hidden="true">{mark}</span> : null}
      <span className="conv-role" title={row.speaker.from ?? undefined}>{name}</span>
      {row.at ? <time className="conv-time" dateTime={row.at}>{formatTime(row.at)}</time> : null}
      <LiveBadges row={row} />
      {row.queued ? <Badge tone="info">Queued · reaches the agent at its next turn</Badge> : null}
      {row.meta ? <MetaLine meta={row.meta} /> : null}
    </header>
  );
}

export const MessageView = memo(function MessageView({ row, current = false }: { readonly row: ConversationRow; readonly current?: boolean }) {
  const live = row.live !== null;
  invariant(row.key !== "", "a row has a key");
  return (
    <article className="conv-msg" data-role={row.role} data-speaker={row.speaker.kind} data-live={live ? "true" : undefined}
      data-seq={row.seq ?? undefined} data-invocation={row.invocation ?? undefined} data-current={current ? "true" : undefined} data-queued={row.queued ? "true" : undefined}>
      <MessageHead row={row} />
      <div className="conv-msg-body">
        {row.live?.recovering && row.parts.length === 0 ? <p className="conv-note">Reading the transcript to catch up with the live output…</p> : null}
        {row.parts.map((part, index) => <PartView key={index} part={part} live={live} rowKey={row.key} index={index} />)}
      </div>
    </article>
  );
});

/** A first guess at a row's height in px, before it is drawn and measured. */
export function estimateRow(row: ConversationRow): number {
  let height = 64;
  for (const part of row.parts) {
    if (part.kind === "text") height += 26 * Math.min(60, Math.ceil(part.text.length / 80) + part.text.split("\n").length - 1);
    else height += 40;
  }
  return height;
}
