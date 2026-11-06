// The learner's reply box: closed while a question waits for its own answer or teaching is paused.

import type { aether } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useState, type FormEvent } from "react";
import { invariant } from "./model.ts";

const TEXT_MAX = 20_000;

function placeholderOf(session: aether.NoesisSession, paused: string | null): string {
  if (paused !== null) return paused;
  if (session.open_question != null) return "Answer the question above to go on";
  return session.active_problem ? "Your answer, or ask for a hint" : "Ask, answer or say what is unclear";
}

/** `paused` says why the learner cannot write now; null leaves the box open. */
export function Composer({ session, busy, paused, onSend }: {
  session: aether.NoesisSession; busy: boolean; paused: string | null; onSend: (text: string) => Promise<string | null>;
}) {
  const [text, setText] = useState("");
  const closed = busy || paused !== null || session.open_question != null;
  const trimmed = text.trim();
  invariant(paused === null || paused.trim() !== "", "a paused composer says why");
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (trimmed === "" || closed) return;
    invariant(trimmed.length <= TEXT_MAX, "a message fits the box's limit");
    if ((await onSend(trimmed)) === null) setText("");
  };
  return (
    <form className="learn-composer" onSubmit={(event) => void submit(event)}>
      <label className="ui-visually-hidden" htmlFor="learn-reply">Message your teacher</label>
      <textarea id="learn-reply" className="learn-textarea" rows={2} maxLength={TEXT_MAX} disabled={closed}
        placeholder={placeholderOf(session, paused)} value={text} onChange={(e) => setText(e.target.value)} />
      <Button variant="primary" type="submit" disabled={closed || trimmed === ""}>Send</Button>
    </form>
  );
}
