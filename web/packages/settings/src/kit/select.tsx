import { useId } from "react";
import { invariant } from "./invariant.ts";

export interface SelectOption { readonly value: string; readonly label: string }

/** A labelled native select; the label is linked by id so it reads, and tests type into it, by its text alone. */
export function SelectField({ label, value, options, onChange, className }: {
  readonly label: string;
  readonly value: string;
  readonly options: readonly SelectOption[];
  readonly onChange: (value: string) => void;
  readonly className?: string;
}) {
  const id = useId();
  invariant(options.some((option) => option.value === value), `${label} holds one of its options`);
  return (
    <div className={`st-field${className ? ` ${className}` : ""}`}>
      <label className="ui-field-label" htmlFor={id}>{label}</label>
      <select id={id} className="st-select" value={value} onChange={(event) => onChange(event.target.value)}>
        {options.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}
      </select>
    </div>
  );
}
