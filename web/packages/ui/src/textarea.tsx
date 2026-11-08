import { useId, type Ref, type TextareaHTMLAttributes } from "react";
import { classes, invariant } from "./classes.ts";

export interface TextAreaProps extends Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, "id"> {
  readonly label: string;
  readonly hint?: string | undefined;
  readonly error?: string | undefined;
  /** Hides the label visually; it stays the accessible name (a composer with an obvious purpose). */
  readonly hideLabel?: boolean | undefined;
  readonly ref?: Ref<HTMLTextAreaElement> | undefined;
}

/** A labelled textarea that grows with its text, between `rows` and a comfortable maximum. */
export function TextArea({ label, hint, error, hideLabel = false, className, rows = 3, ...textarea }: TextAreaProps) {
  invariant(label.trim().length > 0, "a textarea needs a label");
  invariant(rows >= 1, "a textarea shows at least one row");
  const id = useId();
  const hintId = hint === undefined ? undefined : `${id}-hint`;
  const errorId = error === undefined ? undefined : `${id}-error`;
  const describedBy = [hintId, errorId].filter(Boolean).join(" ") || undefined;
  return (
    <div className={classes("ui-field ui-textarea", className)} data-invalid={error === undefined ? undefined : "true"}>
      <label className={classes("ui-field-label", hideLabel && "ui-visually-hidden")} htmlFor={id}>{label}</label>
      <textarea {...textarea} id={id} rows={rows} className="ui-field-input ui-textarea-input" aria-invalid={error === undefined ? undefined : true} aria-describedby={describedBy} />
      {hint === undefined ? null : <p className="ui-field-hint" id={hintId}>{hint}</p>}
      {error === undefined ? null : <p className="ui-field-error" id={errorId}>{error}</p>}
    </div>
  );
}
