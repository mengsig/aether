// Open questions with their options and a free-text answer. A gate waits on its question, so answering
// here decides the gate, as the retired app's inbox did.

import { routeHref } from "@aether-ui/core";
import type { archon } from "@aether-ui/protocol";
import { Button } from "@aether-ui/ui";
import { useState, type FormEvent } from "react";
import { useChange, type ArchonAccess } from "./archon.ts";
import { invariant } from "./model.ts";

function QuestionCard({ access, question, title, onAnswered }: {
  readonly access: ArchonAccess; readonly question: archon.Question; readonly title: string | null; readonly onAnswered: () => void;
}) {
  const change = useChange(access);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const answer = async (value: string) => {
    invariant(question.answer == null, "only an open question is answered");
    setBusy(true);
    const result = await change({ cmd: "answer", question: question.id, text: value });
    setBusy(false);
    if (!result.ok) return setError(result.error);
    setError(null);
    onAnswered();
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (text.trim()) void answer(text.trim());
    else setError("Write an answer first, or pick an option.");
  };
  const disabled = busy || access.refusal !== null;
  const label = `Q${question.number ?? ""}`;
  return (
    <li className="queue-question" aria-busy={busy}>
      <p className="queue-question-text"><strong>{label}</strong> {question.text}</p>
      {question.item && title !== null ? <a className="ui-link" href={routeHref("#/archon/items/:item", { item: question.item })}>{title}</a> : null}
      {question.options.length > 0 ? (
        <div className="queue-options" role="group" aria-label={`Options for ${label}`}>
          {question.options.map((option) => (
            <Button key={option} size="sm" disabled={disabled} title={access.refusal ?? undefined} onClick={() => void answer(option)}>{option}</Button>
          ))}
        </div>
      ) : null}
      <form className="queue-answer" onSubmit={submit} noValidate>
        <input className="ui-field-input" aria-label={`Answer ${label}`} value={text} disabled={disabled} placeholder="Write an answer" onChange={(event) => setText(event.target.value)} />
        <Button type="submit" variant="primary" size="sm" disabled={disabled} title={access.refusal ?? undefined}>Send</Button>
      </form>
      {error ? <p role="alert" className="queue-error">{error}</p> : null}
    </li>
  );
}

/** `titles` names each question's item for its link; null leaves the links out, as on the item's own page. */
export function QuestionList({ access, questions, titles, onAnswered }: {
  readonly access: ArchonAccess; readonly questions: readonly archon.Question[];
  readonly titles: ReadonlyMap<string, string> | null; readonly onAnswered: () => void;
}) {
  return (
    <section className="queue-questions" aria-label="Open questions">
      <h2 className="ui-card-title queue-heading">Needs you</h2>
      <ul className="queue-question-list">
        {questions.map((question) => (
          <QuestionCard key={question.id} access={access} question={question} onAnswered={onAnswered}
            title={question.item && titles ? titles.get(question.item) ?? question.item : null} />
        ))}
      </ul>
    </section>
  );
}
