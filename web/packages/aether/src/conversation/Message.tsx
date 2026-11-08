// One transcript row as its view mode draws it: who speaks (the operator on a tonal surface with a
// mark, the agent plain on the page, a notice quietly), then its text, folded reasoning, tool rows
// (each paired with its result), coordination cards, attachments, model and cost.

import { Badge } from "@aether-ui/ui";
import type { ConnId } from "@aether-ui/protocol";
import { createContext, memo, useContext, useId, type ReactNode } from "react";
import { invariant } from "../fleet/invariant.ts";
import { CoordinationCard, FailedLine, isCard } from "./Coordination.tsx";
import { formatBytes, formatCost, formatDuration, formatTime, formatTokens } from "./format.ts";
import { Markdown } from "./markdown.tsx";
import type { ConversationRow, InvocationMeta, Part, Speaker } from "./rows.ts";
import { foldKey, orphanKey, toolKey, useToggle } from "./toggles.ts";
import { OrphanResult, ToolCall } from "./ToolCall.tsx";
import type { ViewPart, ViewRow } from "./view.ts";

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

interface PartProps {
  readonly part: Part;
  readonly live: boolean;
  /** The list's key for the row, so a fold's state follows the row as the list is redrawn. */
  readonly rowKey: string;
  readonly index: number;
}

function PartView({ part, live, rowKey, index }: PartProps): ReactNode {
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

export interface MessageContext {
  readonly conn: ConnId;
  /** The agent that delegated this one, for its task card. */
  readonly parent: string | null;
}

function ViewPartView({ view, context, ...part }: { readonly view: ViewPart; readonly context: MessageContext } & Omit<PartProps, "part">): ReactNode {
  if (view.outcome === "line") return <FailedLine part={view.part} />;
  if (!isCard(view)) return <PartView part={view.part} {...part} />;
  const card = <CoordinationCard view={view} conn={context.conn} parent={context.parent} />;
  // Drawn whole, a card keeps the call it summarises, input and result, one click away.
  if (view.outcome !== "full" || view.part.kind !== "tool") return card;
  return <>{card}<ToolCall part={view.part} live={part.live} toggleKey={toolKey(part.rowKey, part.index, view.part.id)} /></>;
}

function MetaLine({ meta, usage }: { readonly meta: InvocationMeta; readonly usage: boolean }) {
  const bits: string[] = [];
  if (meta.model && usage) bits.push(meta.model);
  if (meta.usage && usage) bits.push(formatCost(meta.usage), `${formatTokens(meta.usage.input_tokens)} in · ${formatTokens(meta.usage.output_tokens)} out`);
  if (meta.durationMs !== null && usage) bits.push(formatDuration(meta.durationMs));
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

function MessageHead({ view }: { readonly view: ViewRow }) {
  const row = view.row;
  const { name, mark } = speakerName(row.speaker, useContext(AgentNameContext));
  const marked = row.speaker.kind === "user" || row.speaker.kind === "agent";
  return (
    <header className="conv-msg-head">
      {marked ? <span className="conv-mark" aria-hidden="true">{mark}</span> : null}
      <span className="conv-role" title={row.speaker.from ?? undefined}>{name}</span>
      {row.at ? <time className="conv-time" dateTime={row.at}>{formatTime(row.at)}</time> : null}
      <LiveBadges row={row} />
      {row.meta ? <MetaLine meta={row.meta} usage={view.usage} /> : null}
    </header>
  );
}

export interface MessageViewProps {
  readonly view: ViewRow;
  readonly context: MessageContext;
  /** The list's key for this row; folds are remembered under it. Defaults to the row's own key. */
  readonly rowKey?: string;
  /** Holds Find's current match. */
  readonly current?: boolean;
}

export const MessageView = memo(function MessageView({ view, context, rowKey, current = false }: MessageViewProps) {
  const row = view.row;
  const live = row.live !== null;
  invariant(row.key !== "", "a row has a key");
  const scope = rowKey ?? row.key;
  return (
    <article className="conv-msg" data-role={row.role} data-speaker={row.speaker.kind} data-slim={view.slim || undefined} data-live={live ? "true" : undefined}
      data-seq={row.seq ?? undefined} data-invocation={row.invocation ?? undefined} data-current={current ? "true" : undefined}>
      {view.slim ? null : <MessageHead view={view} />}
      <div className="conv-msg-body">
        {row.live?.recovering && row.parts.length === 0 ? <p className="conv-note">Reading the transcript to catch up with the live output…</p> : null}
        {view.parts.map((part, index) => <ViewPartView key={index} view={part} live={live} context={context} rowKey={scope} index={index} />)}
      </div>
    </article>
  );
});

/** A first guess at a row's height in px, before it is drawn and measured. */
export function estimateRow(view: ViewRow): number {
  let height = view.slim ? 16 : 56;
  for (const { part, outcome } of view.parts) {
    if (outcome !== "full") height += 32;
    else if (part.kind === "text") height += 24 * Math.min(60, Math.ceil(part.text.length / 90) + part.text.split("\n").length - 1);
    else if (part.kind === "attachment") height += 36;
    else height += 44;
  }
  return height;
}
