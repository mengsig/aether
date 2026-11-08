// What the learner can do with a whole session: send it to the knowledge graph or delete it.

import { navigate } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, Dialog } from "@aether-ui/ui";
import { useState } from "react";
import { useNoesisChange, type ChangeResult } from "./data.ts";
import { invariant } from "./model.ts";

export const QUEUED_NOTE = "Queued. The knowledge graph updates when the job finishes.";

/** Queues the session's extraction into the knowledge graph; the reply says how it went. */
export function useExtract(conn: string, session: string): () => Promise<ChangeResult> {
  const change = useNoesisChange(conn);
  invariant(session.length > 0, "an extraction names its session");
  return () => change({ cmd: "noesis_extract", session });
}

export function SessionActions({ conn, session, disabled }: { conn: string; session: aether.NoesisSession; disabled: boolean }) {
  const change = useNoesisChange(conn);
  const extract = useExtract(conn, session.id);
  const [confirming, setConfirming] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const queue = async () => {
    const result = await extract();
    setNote(result.ok ? QUEUED_NOTE : result.error);
  };
  const remove = async () => {
    const result = await change({ cmd: "noesis_delete_session", session: session.id, graph: "preserve" });
    if (!result.ok) return setNote(result.error);
    setConfirming(false);
    navigate("#/learn");
  };
  return (
    <section className="learn-session-actions" aria-label="Session actions">
      <Button disabled={disabled} onClick={() => void queue()}>Add to knowledge graph</Button>
      <Button variant="danger" disabled={disabled} onClick={() => setConfirming(true)}>Delete session…</Button>
      {note !== null && <p className="learn-muted" role="status">{note}</p>}
      <Dialog open={confirming} onClose={() => setConfirming(false)} title="Delete this session?"
        description="Noesis forgets this session: its plan, questions, problems and the evidence attached to it."
        footer={<><Button onClick={() => setConfirming(false)} data-autofocus>Keep it</Button><Button variant="danger" onClick={() => void remove()}>Delete session</Button></>}>
        <p>The conversation itself stays in Aether history, where retention removes it in time. What the knowledge graph learned from it stays too.</p>
      </Dialog>
    </section>
  );
}
