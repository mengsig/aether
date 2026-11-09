// Combo: a text field with suggestions underneath (WAI-ARIA APG editable combobox with list
// autocomplete). The text stays the value; a suggestion only fills it in. For open values that
// still have known candidates: paths, timezones, repository roots.

import { useEffect, useId, useRef, useState, type InputHTMLAttributes, type KeyboardEvent, type ReactNode } from "react";
import { useDismissOnOutside } from "./outside.ts";
import { classes, invariant } from "./classes.ts";
import { moveIndex, type PickerOption } from "./picker-model.ts";

export interface ComboProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "value" | "onChange" | "id" | "size"> {
  readonly label: string;
  readonly value: string;
  readonly onChange: (text: string) => void;
  /** Suggestions for the current text; each id is the text a pick fills in. */
  readonly suggestions: readonly PickerOption[];
  /** Called after a pick, with the option, once the text is set. */
  readonly onPick?: (option: PickerOption) => void;
  readonly hint?: string | undefined;
  readonly error?: string | undefined;
  /** Shown under the list while suggestions load, so an empty list is not mistaken for "none". */
  readonly status?: string | undefined;
  /** A control at the input's end: a Browse button, a Validate action. */
  readonly trailing?: ReactNode | undefined;
  readonly emptyText?: string | undefined;
  readonly className?: string | undefined;
}

export function Combo({ label, value, onChange, suggestions, onPick, hint, error, status, trailing, emptyText, className, ...input }: ComboProps) {
  invariant(label.trim().length > 0, "a combo needs a label");
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [cursor, setActive] = useState(-1);
  // New suggestions can be shorter than the old cursor; past their end nothing is active.
  const active = cursor < suggestions.length ? cursor : -1;
  const listed = open && (suggestions.length > 0 || status !== undefined || emptyText !== undefined);
  const activeId = active >= 0 && suggestions[active] !== undefined ? `${id}-option-${active}` : undefined;

  useDismissOnOutside(root, open, () => setOpen(false));

  const pick = (option: PickerOption) => {
    if (option.disabled) return;
    onChange(option.id);
    setOpen(false);
    setActive(-1);
    onPick?.(option);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (!open) setOpen(true);
      setActive(moveIndex(suggestions, active, event.key === "ArrowDown" ? 1 : -1));
    } else if (event.key === "Enter" && open && suggestions[active] !== undefined) {
      event.preventDefault();
      pick(suggestions[active] as PickerOption);
    } else if (event.key === "Escape" && open) {
      event.preventDefault();
      setOpen(false);
    } else if (event.key === "Tab") setOpen(false);
  };
  const hintId = hint === undefined ? undefined : `${id}-hint`;
  const errorId = error === undefined ? undefined : `${id}-error`;
  const describedBy = [hintId, errorId].filter(Boolean).join(" ") || undefined;
  return (
    <div className={classes("ui-field ui-combo", className)} ref={root} data-invalid={error === undefined ? undefined : "true"}>
      <label className="ui-field-label" htmlFor={id}>{label}</label>
      <div className="ui-combo-row">
        <input {...input} id={id} className="ui-field-input ui-combo-input" role="combobox" aria-autocomplete="list" aria-expanded={listed}
          aria-controls={listed ? `${id}-list` : undefined} aria-activedescendant={listed ? activeId : undefined} aria-describedby={describedBy}
          aria-invalid={error === undefined ? undefined : true} autoComplete="off" spellCheck={false} value={value}
          onChange={(event) => { onChange(event.target.value); setOpen(true); setActive(-1); }}
          onFocus={(event) => { setOpen(true); input.onFocus?.(event); }} onClick={() => setOpen(true)} onKeyDown={onKeyDown} />
        {trailing === undefined ? null : <div className="ui-combo-trailing">{trailing}</div>}
      </div>
      {listed ? <ComboList id={id} label={label} suggestions={suggestions} active={active} status={status} emptyText={emptyText} onHover={setActive} onPick={pick} /> : null}
      {hint === undefined ? null : <p className="ui-field-hint" id={hintId}>{hint}</p>}
      {error === undefined ? null : <p className="ui-field-error" id={errorId}>{error}</p>}
    </div>
  );
}

interface ComboListProps {
  readonly id: string;
  readonly label: string;
  readonly suggestions: readonly PickerOption[];
  readonly active: number;
  readonly status: string | undefined;
  readonly emptyText: string | undefined;
  readonly onHover: (index: number) => void;
  readonly onPick: (option: PickerOption) => void;
}

/** The suggestion list under the input; a press on a row never takes focus from the input. */
function ComboList({ id, label, suggestions, active, status, emptyText, onHover, onPick }: ComboListProps) {
  invariant(active >= -1 && active < Math.max(suggestions.length, 1), `combo cursor ${active} is out of range`);
  const list = useRef<HTMLUListElement>(null);
  useEffect(() => {
    const node = list.current?.querySelector<HTMLElement>('[role="option"][data-active="true"]');
    if (node !== undefined && node !== null && typeof node.scrollIntoView === "function") node.scrollIntoView({ block: "nearest" });
  }, [active]);
  return (
    <ul ref={list} className="ui-picker-pop ui-picker-list ui-combo-list" id={`${id}-list`} role="listbox" aria-label={`${label} suggestions`}>
      {suggestions.map((option, index) => (
        <li key={option.id} id={`${id}-option-${index}`} role="option" className="ui-picker-option" aria-selected={index === active} aria-disabled={option.disabled || undefined}
          data-active={index === active ? "true" : undefined} onPointerMove={() => onHover(index)} onMouseDown={(event) => event.preventDefault()} onClick={() => onPick(option)}>
          <span className="ui-picker-option-main">
            <span className="ui-picker-option-label">{option.label}</span>
            {option.badge === undefined ? null : <span className="ui-picker-tag" data-tone={option.badgeTone ?? "neutral"}>{option.badge}</span>}
          </span>
          {option.description === undefined ? null : <span className="ui-picker-option-description">{option.description}</span>}
        </li>
      ))}
      {suggestions.length === 0 && status === undefined && emptyText !== undefined ? <li className="ui-picker-empty" role="presentation">{emptyText}</li> : null}
      {status === undefined ? null : <li className="ui-picker-empty" role="presentation" aria-live="polite">{status}</li>}
    </ul>
  );
}
