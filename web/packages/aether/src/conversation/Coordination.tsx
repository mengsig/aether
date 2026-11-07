// Coordination as cards: a delegation, a message to another agent, a wait, a question or the task
// from the parent, each one line when compact, linking to the other agent's chat and to the task graph.

import { routeHref } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";
import type { RowKind } from "./modes.ts";
import type { Part } from "./rows.ts";
import { canonicalTool, type ViewPart } from "./view.ts";

export interface Directory {
  readonly conn: ConnId;
  readonly agents: Readonly<Record<string, aether.Agent>>;
}

interface Card {
  readonly icon: string;
  readonly title: string;
  readonly text: string;
  /** The other agent, when the card names one this daemon knows of or can name. */
  readonly agent: string | null;
  readonly tone: "accent" | "info" | "neutral";
}

const CARD_KINDS: ReadonlySet<RowKind> = new Set<RowKind>(["spawned", "sent", "waiting", "question", "user_task"]);

/** Whether a part draws as a card; a task's attached image draws as itself. */
export const isCard = (view: ViewPart): boolean => CARD_KINDS.has(view.kind) && view.part.kind !== "attachment";

const AGENT_ID = /\bagt_[0-9A-Za-z]+\b/;

/** A tool's input as the call gave it; a live call's partial JSON reads as no fields. */
function fields(input: string): Readonly<Record<string, unknown>> {
  try {
    const value: unknown = JSON.parse(input);
    return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
  } catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    return {};
  }
}

const str = (value: unknown): string => (typeof value === "string" ? value : Array.isArray(value) ? value.filter((v) => typeof v === "string").join(", ") : "");

function toolCard(kind: RowKind, part: Extract<Part, { kind: "tool" }>, name: (id: string) => string): Card {
  const input = fields(part.input);
  switch (kind) {
    case "spawned": {
      const child = AGENT_ID.exec(part.result?.text ?? "")?.[0] ?? null;
      const who = str(input.name) || (child === null ? str(input.role) || "a new agent" : name(child));
      return { icon: "↳", title: `Delegated to ${who}`, text: str(input.objective), agent: child, tone: "accent" };
    }
    case "sent": {
      const to = str(input.to);
      return { icon: "→", title: `Message to ${to === "" ? "an agent" : name(to)}`, text: str(input.content), agent: AGENT_ID.test(to) ? to : null, tone: "info" };
    }
    case "waiting": {
      const on = str(input.tasks) || str(input.message_from);
      return { icon: "◷", title: on === "" ? "Waiting" : `Waiting on ${on}`, text: "", agent: AGENT_ID.exec(str(input.message_from))?.[0] ?? null, tone: "neutral" };
    }
    default:
      invariant(kind === "question", `${kind} is a tool card`);
      return { icon: "?", title: "Asked the operator", text: str(input.question), agent: null, tone: "accent" };
  }
}

function cardOf(view: ViewPart, directory: Directory, parent: string | null): Card {
  const name = (id: string) => directory.agents[id]?.name ?? id;
  const part = view.part;
  if (part.kind === "tool") return toolCard(view.kind, part, name);
  invariant(part.kind === "text" && view.kind === "user_task", `a ${part.kind} part draws no card`);
  return { icon: "↘", title: parent === null ? "Task" : `Task from ${name(parent)}`, text: part.text, agent: parent, tone: "accent" };
}

export function CoordinationCard({ view, directory, parent }: { readonly view: ViewPart; readonly directory: Directory; readonly parent: string | null }) {
  invariant(isCard(view), `${view.kind} ${view.part.kind} is drawn as a card`);
  const card = cardOf(view, directory, parent);
  const compact = view.outcome === "compact";
  return (
    <div className="conv-card" data-kind={view.kind} data-tone={card.tone} data-compact={compact || undefined}>
      <span className="conv-card-icon" aria-hidden="true">{card.icon}</span>
      <span className="conv-card-title">{card.title}</span>
      {card.text === "" ? null : <span className="conv-card-text" title={compact ? card.text : undefined}>{card.text}</span>}
      <span className="conv-card-links">
        {card.agent === null ? null : <a className="ui-link" href={routeHref("#/agents/:conn/:agent", { conn: directory.conn, agent: card.agent })}>Open chat</a>}
        {view.kind === "spawned" ? <a className="ui-link" href="#/fleet/topology">Graph</a> : null}
      </span>
    </div>
  );
}

/** A failed call in a mode without tools: its name, "failed" and the first line of its error. */
export function FailedLine({ part }: { readonly part: Part }) {
  invariant(part.kind === "tool" || part.kind === "orphan-result", "only a call fails as one line");
  const tool = part.kind === "tool" ? canonicalTool(part.name) : "tool";
  const label = ["bash", "shell", "exec_command", "local_shell", "shell_bg"].includes(tool) ? "Command" : part.kind === "tool" ? part.name : "Tool";
  const first = (part.result?.text ?? "").trim().split("\n", 1)[0] ?? "";
  return (
    <div className="conv-line" data-error="true">
      <span className="conv-card-icon" aria-hidden="true">!</span>
      <span className="conv-card-title"><code>{label}</code> failed</span>
      {first === "" ? null : <span className="conv-card-text">{first}</span>}
    </div>
  );
}
