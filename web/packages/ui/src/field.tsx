import { useId, type InputHTMLAttributes, type Ref } from "react";
import { classes, invariant } from "./classes.ts";

export interface FieldProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "id"> {
  readonly label: string;
  readonly hint?: string;
  /** Shown under the input and announced with it; marks the input invalid. */
  readonly error?: string;
  readonly ref?: Ref<HTMLInputElement>;
}

/** A labelled input whose hint and error are wired to it with aria-describedby. */
export function Field({ label, hint, error, className, ...input }: FieldProps) {
  invariant(label.trim().length > 0, "a field needs a label");
  const id = useId();
  const hintId = hint === undefined ? undefined : `${id}-hint`;
  const errorId = error === undefined ? undefined : `${id}-error`;
  const describedBy = [hintId, errorId].filter(Boolean).join(" ");
  return (
    <div className={classes("ui-field", className)} data-invalid={error === undefined ? undefined : "true"}>
      <label className="ui-field-label" htmlFor={id}>{label}</label>
      <input {...input} id={id} className="ui-field-input" aria-invalid={error === undefined ? undefined : true}
        aria-describedby={describedBy === "" ? undefined : describedBy} />
      {hint === undefined ? null : <p className="ui-field-hint" id={hintId}>{hint}</p>}
      {error === undefined ? null : <p className="ui-field-error" id={errorId}>{error}</p>}
    </div>
  );
}
