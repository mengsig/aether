// Switch and Segmented: the two "choose one" controls that are not lists. A switch is one
// setting on or off; a segmented control is one of a few named states, all visible at once.

import { useId, useRef, type KeyboardEvent, type ReactNode } from "react";
import { classes, invariant } from "./classes.ts";

export interface SwitchProps {
  readonly label: string;
  readonly checked: boolean;
  readonly onChange: (checked: boolean) => void;
  readonly hint?: string | undefined;
  readonly disabled?: boolean | undefined;
  readonly className?: string | undefined;
  readonly "data-testid"?: string | undefined;
}

const TOGGLE_KEYS: ReadonlySet<string> = new Set([" ", "Enter"]);

/** A whole-row switch: the text and the track are one button, so the target is large. */
export function Switch({ label, checked, onChange, hint, disabled = false, className, ...rest }: SwitchProps) {
  invariant(label.trim().length > 0, "a switch needs a label");
  invariant(typeof checked === "boolean", `switch ${label} needs a boolean checked`);
  const id = useId();
  // Space and Enter toggle on keydown; the defaults are held back so the button's own click cannot toggle twice.
  const onKeyDown = (event: KeyboardEvent) => {
    if (!TOGGLE_KEYS.has(event.key)) return;
    event.preventDefault();
    if (!event.repeat && !disabled) onChange(!checked);
  };
  const onKeyUp = (event: KeyboardEvent) => { if (event.key === " ") event.preventDefault(); };
  return (
    <button type="button" role="switch" aria-checked={checked} aria-describedby={hint === undefined ? undefined : `${id}-hint`} disabled={disabled}
      className={classes("ui-switch", className)} data-testid={rest["data-testid"]} onClick={() => onChange(!checked)} onKeyDown={onKeyDown} onKeyUp={onKeyUp}>
      <span className="ui-switch-text">
        <span className="ui-switch-label">{label}</span>
        {hint === undefined ? null : <span className="ui-switch-hint" id={`${id}-hint`}>{hint}</span>}
      </span>
      <span className="ui-switch-track" aria-hidden="true"><span className="ui-switch-thumb" /></span>
    </button>
  );
}

export interface SegmentOption { readonly id: string; readonly label: string; readonly hint?: string | undefined; readonly icon?: ReactNode | undefined; readonly disabled?: boolean }

export interface SegmentedProps {
  readonly label: string;
  readonly options: readonly SegmentOption[];
  readonly value: string;
  readonly onChange: (id: string) => void;
  readonly disabled?: boolean | undefined;
  readonly size?: "sm" | "md" | undefined;
  readonly className?: string | undefined;
  /** Lays the segments out as full-width cards with their hints: a short list of modes. */
  readonly layout?: "row" | "cards" | undefined;
}

/** A radio group drawn as segments: arrows move and choose, Tab leaves the group (WAI-ARIA APG). */
export function Segmented({ label, options, value, onChange, disabled = false, size = "md", className, layout = "row" }: SegmentedProps) {
  invariant(options.length > 1, `segmented ${label} needs at least two options`);
  invariant(new Set(options.map((option) => option.id)).size === options.length, `segmented ${label} repeats an id`);
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const current = options.find((option) => option.id === value) ?? options[0];
  invariant(current !== undefined, "a segment is chosen");
  const onKeyDown = (event: KeyboardEvent) => {
    const step = event.key === "ArrowRight" || event.key === "ArrowDown" ? 1 : event.key === "ArrowLeft" || event.key === "ArrowUp" ? -1 : 0;
    if (step === 0) return;
    event.preventDefault();
    const enabled = options.filter((option) => !option.disabled);
    const at = enabled.findIndex((option) => option.id === current.id);
    const next = enabled[(at + step + enabled.length) % enabled.length];
    if (next === undefined) return;
    onChange(next.id);
    refs.current.get(next.id)?.focus();
  };
  return (
    <div className={classes("ui-segmented", className)} role="radiogroup" aria-label={label} data-size={size} data-layout={layout} onKeyDown={onKeyDown}>
      {options.map((option) => (
        <button key={option.id} type="button" role="radio" className="ui-segment" aria-checked={option.id === current.id} tabIndex={option.id === current.id ? 0 : -1}
          disabled={disabled || option.disabled} ref={(node) => { if (node) refs.current.set(option.id, node); else refs.current.delete(option.id); }}
          onClick={() => onChange(option.id)}>
          {option.icon === undefined ? null : <span className="ui-segment-icon" aria-hidden="true">{option.icon}</span>}
          <span className="ui-segment-label">{option.label}</span>
          {option.hint === undefined || layout !== "cards" ? null : <span className="ui-segment-hint">{option.hint}</span>}
        </button>
      ))}
    </div>
  );
}
