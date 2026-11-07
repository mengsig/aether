import { useId } from "react";
import { invariant } from "./invariant.ts";

export interface SelectOption { readonly value: string; readonly label: string }

/** A labelled native select; the label is linked by id so it reads, and tests type into it, by its text alone. */
export function SelectField({ label, value, options, onChange, className, disabled = false, hint }: {
  readonly label: string;
  readonly value: string;
  readonly options: readonly SelectOption[];
  readonly onChange: (value: string) => void;
  readonly className?: string;
  readonly disabled?: boolean;
  /** Why the choice is held, shown under the field. */
  readonly hint?: string | undefined;
}) {
  const id = useId();
  invariant(options.some((option) => option.value === value), `${label} holds one of its options`);
  invariant(!disabled || (hint ?? "").trim() !== "", `a held ${label} says why`);
  return (
    <div className={`st-field${className ? ` ${className}` : ""}`}>
      <label className="ui-field-label" htmlFor={id}>{label}</label>
      <select id={id} className="st-select" value={value} disabled={disabled} aria-describedby={hint ? `${id}-hint` : undefined} onChange={(event) => onChange(event.target.value)}>
        {options.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
      </select>
      {hint ? <p id={`${id}-hint`} className="st-note">{hint}</p> : null}
    </div>
  );
}
