// A tool call as one compact row (glyph, name, what it was asked, status, duration) that opens on
// click or Enter to show its input and output; an orphan result, whose call is not in the transcript, too.

import { Badge, type Tone } from "@aether-ui/ui";
import { createContext, useContext, useId, useMemo, type ReactNode } from "react";
import { invariant } from "../fleet/invariant.ts";
import { CopyButton } from "./CopyButton.tsx";
import { formatDuration } from "./format.ts";
import type { Part, ToolActivity, ToolResultView } from "./rows.ts";
import { argumentSummary, toolGlyph } from "./tool-summary.ts";
import { useToggle } from "./toggles.ts";

/** Long tool input and output stay readable without one call filling the screen. */
const MAX_TOOL_TEXT = 20_000;
const NO_ACTIVITY: ReadonlyMap<string, ToolActivity> = new Map();

/** Each tool call's run state and duration from its events, by tool_use id. */
export const ToolActivityContext = createContext<ReadonlyMap<string, ToolActivity>>(NO_ACTIVITY);

const clip = (text: string): string => (text.length > MAX_TOOL_TEXT ? `${text.slice(0, MAX_TOOL_TEXT)}\n… (${text.length - MAX_TOOL_TEXT} more characters)` : text);

type ToolPart = Extract<Part, { kind: "tool" }>;
interface ToolState { readonly state: "running" | "ok" | "error" | "pending" | "interrupted"; readonly tone: Tone; readonly label: string }

function toolState(part: ToolPart, live: boolean, activity: ToolActivity | undefined): ToolState {
  if (activity?.state === "interrupted") return { state: "interrupted", tone: "warning", label: "Interrupted" };
  if (part.result) return part.result.isError ? { state: "error", tone: "danger", label: "Failed" } : { state: "ok", tone: "success", label: "Done" };
  if (activity?.state === "running") return { state: "running", tone: "info", label: "Running" };
  if (activity?.state === "error") return { state: "error", tone: "danger", label: "Failed" };
  if (activity?.state === "ok") return { state: "ok", tone: "success", label: "Done" };
  return live ? { state: "running", tone: "info", label: "Writing" } : { state: "pending", tone: "neutral", label: "No result yet" };
}

function ToolText({ label, text, error = false }: { readonly label: string; readonly text: string; readonly error?: boolean }) {
  return (
    <div className="conv-tool-section" data-error={error ? "true" : undefined}>
      <div className="conv-tool-bar">
        <span className="conv-tool-label">{label}</span>
        {text === "" ? null : <CopyButton text={text} what={`the ${label.toLowerCase()}`} />}
      </div>
      {text === "" ? <span className="conv-note">Empty</span> : <pre className="conv-pre">{clip(text)}</pre>}
    </div>
  );
}

function ToolResult({ result }: { readonly result: ToolResultView }) {
  return (
    <div className="conv-tool-result" data-error={result.isError ? "true" : undefined}>
      <ToolText label={result.isError ? "Error" : "Output"} text={result.text} error={result.isError} />
      {result.images > 0 ? <span className="conv-note">{result.images} image{result.images === 1 ? "" : "s"}</span> : null}
    </div>
  );
}

interface FoldProps {
  readonly toggleKey: string;
  readonly glyph: string;
  readonly name: string;
  readonly summary: string;
  readonly state: ToolState;
  readonly durationMs: number | null;
  readonly children: ReactNode;
}

/** The row and its panel; the panel stays in the document while folded so its text is still searchable. */
function ToolFold({ toggleKey, glyph, name, summary, state, durationMs, children }: FoldProps) {
  const [open, flip] = useToggle(toggleKey);
  const panel = useId();
  invariant(name !== "", "a tool row has a name");
  return (
    <>
      <button type="button" className="conv-tool-row" data-testid="conv-tool-toggle" aria-expanded={open} aria-controls={panel} onClick={flip}>
        <span className="conv-tool-glyph" aria-hidden="true">{glyph}</span>
        <code className="conv-tool-name">{name}</code>
        {summary === "" ? null : <span className="conv-tool-args">{summary}</span>}
        <span className="conv-tool-end">
          {durationMs === null ? null : <span className="conv-tool-time">{formatDuration(durationMs)}</span>}
          <Badge tone={state.tone}>{state.label}</Badge>
          <span className="conv-chevron" aria-hidden="true" />
        </span>
      </button>
      <div className="conv-tool-panel" id={panel} data-testid="conv-tool-output" hidden={!open}>{children}</div>
    </>
  );
}

export function ToolCall({ part, live, toggleKey }: { readonly part: ToolPart; readonly live: boolean; readonly toggleKey: string }) {
  invariant(part.kind === "tool", "a tool row draws a tool call");
  invariant(toggleKey.startsWith("t:"), `tool row toggle ${toggleKey} is a tool key`);
  const activity = useContext(ToolActivityContext).get(part.id ?? "");
  const state = toolState(part, live, activity);
  const summary = useMemo(() => argumentSummary(part.input), [part.input]);
  const name = part.name || "tool";
  return (
    <div className="conv-tool" data-tool-use-id={part.id ?? undefined} data-state={state.state}>
      <ToolFold toggleKey={toggleKey} glyph={toolGlyph(name)} name={name} summary={summary} state={state} durationMs={activity?.durationMs ?? null}>
        <ToolText label="Input" text={part.input} />
        {part.result ? <ToolResult result={part.result} /> : null}
      </ToolFold>
    </div>
  );
}

export function OrphanResult({ part, toggleKey }: { readonly part: Extract<Part, { kind: "orphan-result" }>; readonly toggleKey: string }) {
  const state: ToolState = part.result.isError ? { state: "error", tone: "danger", label: "Failed" } : { state: "ok", tone: "success", label: "Done" };
  return (
    <div className="conv-tool conv-orphan" data-state={state.state}>
      <ToolFold toggleKey={toggleKey} glyph="↩" name="result" summary={`for ${part.toolUseId}`} state={state} durationMs={null}>
        <ToolResult result={part.result} />
      </ToolFold>
    </div>
  );
}
