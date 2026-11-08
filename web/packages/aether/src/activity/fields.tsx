import { useId, type ReactNode } from "react";

interface FieldShellProps { readonly label: string; readonly hint?: string; readonly testId?: string }

/** A multiline text field in the kit's field style; `data-autofocus` lets the sheet focus it on open. */
export function TextAreaField({ label, hint, testId, value, rows = 4, disabled, autoFocus, onChange }: FieldShellProps & {
  readonly value: string; readonly rows?: number; readonly disabled?: boolean; readonly autoFocus?: boolean; readonly onChange: (value: string) => void;
}) {
  const hintId = useId();
  return <label className="b3-field"><span className="ui-field-label">{label}</span>
    <textarea className="b3-textarea" rows={rows} value={value} disabled={disabled} data-autofocus={autoFocus ? "" : undefined} data-testid={testId}
      aria-describedby={hint === undefined ? undefined : hintId} onChange={e => onChange(e.target.value)} />
    {hint === undefined ? null : <span className="ui-field-hint" id={hintId}>{hint}</span>}</label>;
}

/** A single choice from a native list, in the kit's field style. */
export function SelectField({ label, hint, testId, value, disabled, onChange, children }: FieldShellProps & {
  readonly value: string; readonly disabled?: boolean; readonly onChange: (value: string) => void; readonly children: ReactNode;
}) {
  const hintId = useId();
  return <label className="b3-field"><span className="ui-field-label">{label}</span>
    <select className="b3-select" value={value} disabled={disabled} data-testid={testId} aria-describedby={hint === undefined ? undefined : hintId} onChange={e => onChange(e.target.value)}>{children}</select>
    {hint === undefined ? null : <span className="ui-field-hint" id={hintId}>{hint}</span>}</label>;
}
