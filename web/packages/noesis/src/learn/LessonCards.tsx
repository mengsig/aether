// The cards around a lesson's chat: the work it started from, what it cost, and the end of the path.

import { navigate, routeHref } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useState } from "react";
import { useNoesisChange } from "./data.ts";
import { billingWords, invariant, planRows } from "./model.ts";
import { QUEUED_NOTE, useExtract } from "./SessionActions.tsx";

const STATUS_LABEL: Readonly<Record<aether.EvidenceStatus, string>> = { verified: "Verified", reviewed: "Reviewed", preliminary: "Preliminary" };

/** The evidence a lesson started from, compact: one line per source with its standing. */
export function SourcesCard({ bundle }: { bundle: aether.EvidenceBundle }) {
  invariant(bundle.items.length > 0, "a sources card has sources");
  return (
    <details className="learn-lesson-sources">
      <summary>Sources <span className="learn-muted">· {bundle.title} · {bundle.items.length}</span></summary>
      <ul className="learn-lesson-sources-list">
        {bundle.items.map((item) => (
          <li key={`${item.ref.kind}:${item.ref.id}`}>
            <span className="learn-standing" data-status={item.status}>{STATUS_LABEL[item.status]}</span>{" "}
            {item.ref.url ? <a className="ui-link" href={item.ref.url} target="_blank" rel="noreferrer">{item.ref.kind} {item.ref.id}</a> : <span>{item.ref.kind} {item.ref.id}</span>}
            <p className="learn-muted learn-lesson-sources-excerpt">{item.excerpt.split("\n", 1)[0]}</p>
          </li>
        ))}
      </ul>
    </details>
  );
}

/** What paid for this lesson, as the session read reports it. */
export function UsageNote({ usage }: { usage: aether.SessionUsage | null | undefined }) {
  return <p className="learn-muted learn-usage" aria-label="Lesson billing">{billingWords(usage)}</p>;
}

/** Shown once every step of the plan is done: what was covered and where to go next. */
export function CompletionCard({ conn, session }: { conn: string; session: aether.NoesisSession }) {
  const change = useNoesisChange(conn);
  const extract = useExtract(conn, session.id);
  const [note, setNote] = useState<string | null>(null);
  invariant(session.plan != null && session.plan.nodes.every((node) => node.status === "visited"), "a lesson completes when its plan is done");
  const steps = planRows(session.plan);
  const practise = async () => {
    const result = await change({ cmd: "noesis_create_session", goal: session.goal, mode: "practice" });
    if (!result.ok) return setNote(result.error);
    const data = result.reply.data as { id?: unknown } | undefined;
    invariant(typeof data?.id === "string", "noesis_create_session answers with the new session");
    navigate(routeHref("#/learn/sessions/:id", { id: data.id }));
  };
  const remember = async () => {
    const result = await extract();
    setNote(result.ok ? QUEUED_NOTE : result.error);
  };
  return (
    <section className="learn-complete" aria-label="Lesson complete">
      <h3 className="learn-callout-title">You finished this lesson</h3>
      <p className="learn-muted">You worked through {steps.length === 1 ? "its one step" : `all ${steps.length} steps`}:</p>
      <ul className="learn-complete-steps">{steps.map(({ node }) => <li key={node.id}>{node.title}</li>)}</ul>
      <div className="learn-complete-actions">
        {session.mode !== "practice" && <Button variant="primary" onClick={() => void practise()}>Practise</Button>}
        <Button onClick={() => void remember()}>Add to knowledge graph</Button>
        <Button onClick={() => navigate("#/learn")}>Done</Button>
      </div>
      {note !== null && <p className="learn-muted" role="status">{note}</p>}
    </section>
  );
}
