// One transcript row: a committed message or a live tail, with its reasoning, tool calls
// (each paired with its result), attachments, model and cost.

import { Badge } from "@aether-ui/ui";
import { memo, type ReactNode } from "react";
import { formatBytes, formatCost, formatDuration, formatTime, formatTokens } from "./format.ts";
import { Markdown } from "@aether-ui/ui";
import type { ConversationRow, InvocationMeta, Part, ToolResultView } from "./rows.ts";

/** Long tool input and output stay readable without one call filling the screen. */
const MAX_TOOL_TEXT = 20_000;

const clip = (text: string): string => (text.length > MAX_TOOL_TEXT ? `${text.slice(0, MAX_TOOL_TEXT)}\n… (${text.length - MAX_TOOL_TEXT} more characters)` : text);

function ToolResult({ result }: { readonly result: ToolResultView }) {
  return (
    <div className="conv-tool-result" data-error={result.isError ? "true" : undefined}>
      <span className="conv-tool-label">{result.isError ? "Error" : "Result"}</span>
      {result.text === "" ? null : <pre className="conv-pre">{clip(result.text)}</pre>}
      {result.images > 0 ? <span className="conv-note">{result.images} image{result.images === 1 ? "" : "s"}</span> : null}
    </div>
  );
}

function toolState(part: Extract<Part, { kind: "tool" }>, live: boolean): { tone: "info" | "success" | "danger" | "neutral"; label: string } {
  if (part.result) return part.result.isError ? { tone: "danger", label: "Failed" } : { tone: "success", label: "Done" };
  return live ? { tone: "info", label: "Writing" } : { tone: "neutral", label: "No result yet" };
}

function ToolCall({ part, live }: { readonly part: Extract<Part, { kind: "tool" }>; readonly live: boolean }) {
  const state = toolState(part, live);
  return (
    <details className="conv-tool" data-tool-use-id={part.id ?? undefined}>
      <summary className="conv-tool-summary">
        <code className="conv-tool-name">{part.name || "tool"}</code>
        <Badge tone={state.tone}>{state.label}</Badge>
      </summary>
      <span className="conv-tool-label">Input</span>
      <pre className="conv-pre">{clip(part.input)}</pre>
      {part.result ? <ToolResult result={part.result} /> : null}
    </details>
  );
}

function PartView({ part, live }: { readonly part: Part; readonly live: boolean }): ReactNode {
  switch (part.kind) {
    case "text": return <Markdown text={part.text} />;
    case "reasoning":
      return (
        <details className="conv-reasoning">
          <summary>{part.redacted ? "Reasoning (redacted)" : "Reasoning"}</summary>
          {part.redacted ? null : <p className="conv-reasoning-text">{part.text}</p>}
        </details>
      );
    case "tool": return <ToolCall part={part} live={live} />;
    case "orphan-result":
      return (
        <div className="conv-tool conv-orphan">
          <span className="conv-tool-label">Result for <code>{part.toolUseId}</code></span>
          <ToolResult result={part.result} />
        </div>
      );
    case "attachment":
      return (
        <span className="conv-attachment">
          <span className="conv-attachment-name">{part.name}</span>
          <span className="conv-note">{part.mediaType}{part.sizeBytes === null ? "" : ` · ${formatBytes(part.sizeBytes)}`}</span>
        </span>
      );
    case "compaction":
      return (
        <details className="conv-reasoning conv-compaction">
          <summary>Earlier conversation compacted</summary>
          <p className="conv-reasoning-text">{part.text}</p>
        </details>
      );
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

export const MessageView = memo(function MessageView({ row }: { readonly row: ConversationRow }) {
  const live = row.live !== null;
  return (
    <article className="conv-msg" data-role={row.role} data-live={live ? "true" : undefined} data-seq={row.seq ?? undefined} data-invocation={row.invocation ?? undefined}>
      <header className="conv-msg-head">
        <span className="conv-role">{row.role === "assistant" ? "Assistant" : "User"}</span>
        {row.at ? <time className="conv-note" dateTime={row.at}>{formatTime(row.at)}</time> : null}
        <LiveBadges row={row} />
        {row.meta ? <MetaLine meta={row.meta} /> : null}
      </header>
      <div className="conv-msg-body">
        {row.live?.recovering && row.parts.length === 0 ? <p className="conv-note">Reading the transcript to catch up with the live output…</p> : null}
        {row.parts.map((part, index) => <PartView key={index} part={part} live={live} />)}
      </div>
    </article>
  );
});

/** A first guess at a row's height in px, before it is drawn and measured. */
export function estimateRow(row: ConversationRow): number {
  let height = 56;
  for (const part of row.parts) {
    if (part.kind === "text") height += 24 * Math.min(60, Math.ceil(part.text.length / 90) + part.text.split("\n").length - 1);
    else if (part.kind === "attachment") height += 36;
    else height += 44;
  }
  return height;
}
