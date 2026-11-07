// What retrieval did for each model call of a task, or for one invocation (MEMORY-V2.md §2.1).
// The trace was recorded when the turn was built, so it says what happened then.

import { connectionDraftKey, useDraft, useReread, selectEvents, useSelector } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { Button, Combo, Segmented } from "@aether-ui/ui";
import { useState, type FormEvent } from "react";
import { invariant } from "../fleet/invariant.ts";
import { ReadView, Section } from "../controls/kit.tsx";
import { ExplainTurn } from "./ExplainTurn.tsx";
import { useHostGate } from "./v2.ts";

/** What to explain; `asked` counts submits, so asking again for the same id reads it again. */
type Target = { readonly kind: "task" | "invocation"; readonly id: string; readonly asked: number };
const PREFIX = { task: "tsk_", invocation: "inv_" } as const;
const LABEL = { task: "Task id", invocation: "Invocation id" } as const;

export function ExplainSection({ conn, known }: { conn: ConnId; known: ReadonlyMap<string, string> }) {
  const form = useDraft(connectionDraftKey("memory", conn, "explain"), { kind: "task" as Target["kind"], text: "" });
  const { kind, text: draft } = form.value;
  const events = useSelector(selectEvents(conn));
  const ids = events.flatMap(e => kind === "task" ? (e.task_id ? [e.task_id] : []) : e.kind.type === "invocation.completed" ? [e.kind.invocation] : []);
  const suggestions = [...new Set(ids)].map(id => ({ id, label: id }));
  const [target, setTarget] = useState<Target | null>(null);
  const gate = useHostGate(conn, { cmd: "memory_retrieval_explain", task: null });
  const id = draft.trim();
  const malformed = id !== "" && !id.startsWith(PREFIX[kind]) ? `${LABEL[kind]}s start with ${PREFIX[kind]}` : undefined;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (id === "" || malformed) return;
    setTarget(previous => ({ kind, id, asked: (previous?.asked ?? 0) + 1 }));
  };
  return <Section title="Retrieval explain" hint="Host only · Which memories each turn was given, and why the rest were not.">
    {gate.refusal ? <p role="note" className="b3-note">{gate.refusal}</p> : <>
      <Segmented label="Explain by" value={kind} onChange={next => form.update({ kind: next as Target["kind"], text: "" })}
        options={[{ id: "task", label: "Task" }, { id: "invocation", label: "Invocation" }]} />
      <form className="mem-inline-form" onSubmit={submit}>
        <Combo label={LABEL[kind]} suggestions={suggestions} value={draft} onChange={text => form.update({ text })} {...(malformed ? { error: malformed } : {})} spellCheck={false} autoComplete="off" />
        <Button type="submit" disabled={id === "" || malformed !== undefined}>Explain</Button>
      </form>
      {target ? <ExplainResult conn={conn} target={target} known={known} /> : <p className="b3-meta">Name a task or an invocation to see its turns, with every memory retrieval weighed.</p>}
    </>}
  </Section>;
}

function ExplainResult({ conn, target, known }: { conn: ConnId; target: Target; known: ReadonlyMap<string, string> }) {
  invariant(target.id.startsWith(PREFIX[target.kind]), "the target id was checked before it was submitted");
  const command = target.kind === "task" ? { cmd: "memory_retrieval_explain", task: target.id } : { cmd: "memory_retrieval_explain", invocation: target.id };
  const [again, setAgain] = useState(0);
  // One store key per target, so a new target never shows the previous one's turns.
  const entry = useReread(conn, `memory:explain:${target.kind}:${target.id}`, command, ["memory_retrieval_explain"], `${target.asked}:${again}`);
  return <ReadView entry={entry} what="the retrieval trace" empty="No turns recorded for it yet" guard={isExplain}
    isEmpty={value => value.turns.length === 0} retry={() => setAgain(n => n + 1)}>
    {value => <div className="b3-stack">
      {value.truncated ? <p className="b3-note" role="note">Only the first 256 snapshots are shown.</p> : null}
      {value.turns.map(turn => <ExplainTurn key={turn.snapshot} turn={turn} known={known} />)}
    </div>}
  </ReadView>;
}

function isExplain(value: unknown): value is aether.RetrievalExplain {
  return value !== null && typeof value === "object" && Array.isArray((value as { turns?: unknown }).turns);
}
