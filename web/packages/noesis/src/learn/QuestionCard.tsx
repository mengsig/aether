// One posed question, for every kind the protocol defines (choice, recall, goal). The answer key is
// never on the wire before the answer is recorded; once it is, the card shows the learner's answer
// beside the daemon's reveal: right or wrong, the key and why.

import { useDraft } from "@aether-ui/core";
import type { aether } from "@aether-ui/protocol";
import { Button, InlineMarkdown, Markdown, Segmented, TextArea } from "@aether-ui/ui";
import { useId, useState, type FormEvent } from "react";
import { CONFIDENCE_STOPS, draftKey, invariant, type Confidence } from "./model.ts";

export interface AnswerInput {
  readonly choices?: string[];
  readonly text?: string;
  readonly dont_know: boolean;
  readonly confidence?: Confidence;
  readonly thinking?: string;
}

const KIND_LABEL: Readonly<Record<aether.QuestionKind, string>> = { choice: "Choose", recall: "Recall", goal: "Your goal" };

export interface QuestionCardProps {
  readonly question: aether.Question;
  /** The learner's answer as the session read recorded it. */
  readonly answer: aether.RecordedAnswer | undefined;
  readonly result: aether.QuestionResult | undefined;
  readonly open: boolean;
  /** Absent when this device may not answer. */
  readonly onAnswer?: ((answer: AnswerInput) => Promise<string | null>) | undefined;
}

export function QuestionCard({ question, answer, result, open: posedOpen, onAnswer }: QuestionCardProps) {
  invariant(question.kind !== "choice" || (question.options?.length ?? 0) >= 2, "a choice question has at least two options");
  // A result event can land before the session's reread closes the question: the result wins.
  const open = posedOpen && result === undefined && answer === undefined;
  const answered = !open;
  const body = (
    <>
      <div className="learn-q-prompt"><Markdown text={question.prompt} /></div>
      {open && <AnswerForm question={question} onAnswer={onAnswer} />}
      {answered && <Reveal question={question} answer={answer} result={result} />}
    </>
  );
  return (
    <article className="learn-q" data-state={open ? "open" : result?.correct === true ? "correct" : result?.correct === false ? "incorrect" : "answered"} aria-label={`${KIND_LABEL[question.kind]} question`}>
      <p className="learn-q-kind">{KIND_LABEL[question.kind]}{question.multi ? " · pick every one that fits" : ""}</p>
      {answered && question.collapsed ? (
        <details className="learn-q-details">
          <summary><InlineMarkdown text={question.prompt} /><span className="learn-q-summary-result"> · {resultWord(result)}</span></summary>
          <Reveal question={question} answer={answer} result={result} />
        </details>
      ) : body}
    </article>
  );
}

function resultWord(result: aether.QuestionResult | undefined): string {
  if (result?.correct === true) return "Correct";
  if (result?.correct === false) return "Not quite";
  return "Answered";
}

/** The option labels for `values` in the order they were shown; a value no option carries reads as itself. */
function labelsOf(question: aether.Question, values: readonly string[]): string[] {
  const options = question.options ?? [];
  return [...options.filter((option) => values.includes(option.value)).map((option) => option.label), ...values.filter((value) => !options.some((option) => option.value === value))];
}

/** What the learner answered, in their words or the options they chose, with how sure they were. */
function GivenAnswer({ answer }: { answer: aether.RecordedAnswer }) {
  invariant(answer.dont_know || (answer.chosen ?? []).length > 0 || (answer.text ?? "") !== "", "a recorded answer says something");
  const sure = answer.confidence == null ? "" : ` · ${answer.confidence}% sure`;
  if (answer.dont_know) return <p className="learn-reveal-given"><span className="learn-muted">You said you did not know{sure}</span></p>;
  const given = (answer.chosen ?? []).length > 0 ? (answer.chosen ?? []).join(", ") : answer.text ?? "";
  return <p className="learn-reveal-given"><span className="learn-muted">Your answer: </span><InlineMarkdown text={given} /><span className="learn-muted">{sure}</span></p>;
}

/** The daemon's reveal beside the learner's answer: right or wrong, the key in the labels they saw, and why. */
function Reveal({ question, answer, result }: { question: aether.Question; answer: aether.RecordedAnswer | undefined; result: aether.QuestionResult | undefined }) {
  if (result === undefined) {
    return <div className="learn-reveal">{answer && <GivenAnswer answer={answer} />}<p className="learn-muted" aria-busy="true">Your answer is in. Its result shows here in a moment.</p></div>;
  }
  invariant(result.question === question.id, "a result belongs to its question");
  const keys = result.answer_key == null ? [] : Array.isArray(result.answer_key) ? result.answer_key : [result.answer_key];
  const labels = labelsOf(question, keys);
  const wrong = result.correct === false;
  return (
    <div className="learn-reveal" role="status">
      {result.correct != null && <p className="learn-reveal-verdict" data-correct={result.correct}>{result.correct ? "✓ Correct" : "✗ Not quite"}</p>}
      {question.kind === "recall" && result.correct == null && !answer?.dont_know && <p className="learn-muted">Awaiting the teacher’s judgment.</p>}
      {answer && <GivenAnswer answer={answer} />}
      {labels.length > 0 && <p><span className="learn-muted">{wrong ? "Correct answer: " : "Answer: "}</span>{labels.map((label, index) => <span key={index}>{index > 0 ? ", " : ""}<InlineMarkdown text={label} /></span>)}</p>}
      {result.misconception && <p><span className="learn-muted">Misconception: </span><InlineMarkdown text={result.misconception} /></p>}
      {result.explanation && <div className="learn-reveal-why">{wrong && <span className="learn-muted">Why: </span>}<Markdown text={result.explanation} /></div>}
    </div>
  );
}

const EMPTY_ANSWER: { readonly text: string; readonly thinking: string } = { text: "", thinking: "" };

function AnswerForm({ question, onAnswer }: { question: aether.Question; onAnswer: QuestionCardProps["onAnswer"] }) {
  const id = useId();
  const draft = useDraft(draftKey(question.id, "answer"), EMPTY_ANSWER);
  const [choices, setChoices] = useState<readonly string[]>([]);
  const [confidence, setConfidence] = useState<Confidence>(50);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const { text, thinking } = draft.value;
  const graded = question.kind !== "goal";
  const ready = question.kind === "choice" ? choices.length > 0 : text.trim() !== "";
  const send = async (dontKnow: boolean) => {
    if (onAnswer === undefined) return;
    setBusy(true);
    const answer: AnswerInput = dontKnow ? { dont_know: true } : {
      dont_know: false,
      ...(question.kind === "choice" ? { choices: [...choices] } : { text: text.trim() }),
      ...(graded ? { confidence } : {}),
      ...(thinking.trim() !== "" ? { thinking: thinking.trim() } : {}),
    };
    const problem = await onAnswer(answer);
    setBusy(false);
    setFailure(problem);
    if (problem === null) draft.clearIfUnchanged();
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (ready) void send(false);
  };
  const pick = (value: string, on: boolean) =>
    setChoices((old) => (question.multi ? (on ? [...old, value] : old.filter((v) => v !== value)) : [value]));
  return (
    <form className="learn-q-form" onSubmit={submit}>
      {question.kind === "choice"
        ? <ChoiceOptions name={`${id}-choice`} question={question} chosen={choices} onPick={pick} />
        : <TextArea label={question.kind === "goal" ? "Your goal, in your words" : "Your answer"} hideLabel data-testid="learn-answer" rows={3}
            placeholder={question.kind === "goal" ? "Describe it in your own words" : "Answer from memory"} value={text} onChange={(e) => draft.update({ text: e.target.value })} />}
      {graded && <ConfidencePicker value={confidence} onChange={setConfidence} />}
      <details className="learn-thinking" open={thinking !== "" || undefined}>
        <summary>Add how you got there</summary>
        <TextArea label="Your reasoning" hideLabel rows={2} value={thinking} onChange={(e) => draft.update({ thinking: e.target.value })} />
      </details>
      {failure !== null && <p className="learn-callout" role="alert">{failure}</p>}
      <div className="learn-actions">
        {graded && <Button variant="quiet" disabled={busy || onAnswer === undefined} onClick={() => void send(true)}>I don't know</Button>}
        <Button variant="primary" type="submit" disabled={busy || !ready || onAnswer === undefined}>{busy ? "Sending…" : "Answer"}</Button>
      </div>
    </form>
  );
}

/** The options as cards; each keeps its native radio or checkbox, so arrows, Space and screen readers work as ever. */
function ChoiceOptions({ name, question, chosen, onPick }: { name: string; question: aether.Question; chosen: readonly string[]; onPick: (value: string, on: boolean) => void }) {
  const options = question.options ?? [];
  invariant(new Set(options.map((option) => option.value)).size === options.length, "choice options have distinct values");
  return (
    <fieldset className="learn-options">
      <legend className="ui-visually-hidden">Options</legend>
      {options.map((option, index) => (
        <label key={option.value} className="learn-option" data-multi={question.multi || undefined}>
          <input type={question.multi ? "checkbox" : "radio"} name={name} value={option.value} checked={chosen.includes(option.value)}
            onChange={(e) => onPick(option.value, e.target.checked)} />
          <span className="learn-option-mark" aria-hidden="true">{String.fromCharCode(65 + (index % 26))}</span>
          <span className="learn-option-label"><InlineMarkdown text={option.label} /></span>
        </label>
      ))}
    </fieldset>
  );
}

export const CONFIDENCE_OPTIONS = CONFIDENCE_STOPS.map((stop) => ({ id: String(stop), label: `${stop}%` }));

function ConfidencePicker({ value, onChange }: { value: Confidence; onChange: (value: Confidence) => void }) {
  const choose = (id: string) => {
    const stop = CONFIDENCE_STOPS.find((candidate) => String(candidate) === id);
    invariant(stop !== undefined, `confidence ${id} is one of the stops`);
    onChange(stop);
  };
  return (
    <div className="learn-confidence">
      <span className="learn-muted" aria-hidden="true">How sure are you?</span>
      <Segmented label="How sure are you?" size="sm" options={CONFIDENCE_OPTIONS} value={String(value)} onChange={choose} />
    </div>
  );
}
