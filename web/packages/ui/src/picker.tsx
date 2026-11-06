// Picker: a select whose list is a real, styled listbox (WAI-ARIA APG select-only combobox).
// Arrows move, type-ahead jumps, Enter chooses, Escape closes; a long list gets a search box.
// It changes only how a value is chosen: `onChange` receives the option's id, nothing else.

import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { classes, invariant } from "./classes.ts";
import { filterOptions, groupOptions, moveIndex, typeAhead, type PickerOption } from "./picker-model.ts";

export interface PickerProps {
  readonly label: string;
  /** The chosen option's id; null or an unknown id shows the placeholder. */
  readonly value: string | null;
  readonly onChange: (id: string) => void;
  readonly options: readonly PickerOption[];
  readonly placeholder?: string | undefined;
  readonly hint?: string | undefined;
  readonly error?: string | undefined;
  readonly disabled?: boolean | undefined;
  /** A search box above the list; on by default past SEARCH_FROM options. */
  readonly searchable?: boolean | undefined;
  readonly emptyText?: string | undefined;
  readonly className?: string | undefined;
  readonly size?: "sm" | "md" | undefined;
  /** Hides the label visually; it stays the accessible name. */
  readonly hideLabel?: boolean | undefined;
  readonly "data-testid"?: string | undefined;
}

export const SEARCH_FROM = 8;
const TYPEAHEAD_RESET_MS = 600;

interface ListState {
  readonly open: boolean;
  readonly active: number;
  readonly query: string;
}

const CLOSED: ListState = { open: false, active: -1, query: "" };

export function Picker(props: PickerProps) {
  const { label, value, onChange, options, disabled = false, error, hint } = props;
  invariant(label.trim().length > 0, "a picker needs a label");
  const id = useId();
  const [state, setState] = useState<ListState>(CLOSED);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const typed = useRef<{ text: string; at: number }>({ text: "", at: 0 });
  const searchable = props.searchable ?? options.length >= SEARCH_FROM;
  const shown = state.query === "" ? options : filterOptions(options, state.query);
  const selected = options.find((option) => option.id === value);

  useEffect(() => {
    if (!state.open) return undefined;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setState(CLOSED);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [state.open]);

  const open = () => {
    if (disabled) return;
    const at = shown.findIndex((option) => option.id === value && !option.disabled);
    setState({ open: true, active: at >= 0 ? at : moveIndex(shown, -1, 1), query: "" });
  };
  const close = (refocus: boolean) => {
    setState(CLOSED);
    if (refocus) trigger.current?.focus();
  };
  const choose = (option: PickerOption) => {
    if (option.disabled) return;
    close(true);
    if (option.id !== value) onChange(option.id);
  };
  const move = (active: number) => setState((current) => ({ ...current, active }));
  const typeKey = (key: string) => {
    const now = Date.now();
    const text = now - typed.current.at > TYPEAHEAD_RESET_MS ? key : typed.current.text + key;
    typed.current = { text, at: now };
    const found = typeAhead(shown, state.active, text);
    if (found >= 0) move(found);
  };

  const onListKey = (event: KeyboardEvent) => {
    const handled = listKey(event, { shown, active: state.active, move, choose, close, typeKey, searchable });
    if (handled) event.preventDefault();
  };
  const onTriggerKey = (event: KeyboardEvent) => {
    if (state.open) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      open();
    } else if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
      const found = typeAhead(options, options.findIndex((option) => option.id === value), event.key);
      if (found >= 0) choose(options[found] as PickerOption);
    }
  };

  const hintId = hint === undefined ? undefined : `${id}-hint`;
  const errorId = error === undefined ? undefined : `${id}-error`;
  const describedBy = [hintId, errorId].filter(Boolean).join(" ") || undefined;
  return (
    <div className={classes("ui-field ui-picker", props.className)} ref={root} data-invalid={error === undefined ? undefined : "true"} data-size={props.size ?? "md"}>
      <label className={classes("ui-field-label", props.hideLabel && "ui-visually-hidden")} id={`${id}-label`} htmlFor={`${id}-trigger`}>{label}</label>
      <button type="button" className="ui-field-input ui-picker-trigger" id={`${id}-trigger`} ref={trigger} role="combobox" disabled={disabled}
        aria-haspopup="listbox" aria-expanded={state.open} aria-controls={state.open ? `${id}-list` : undefined} aria-labelledby={`${id}-label ${id}-trigger`}
        aria-describedby={describedBy} aria-invalid={error === undefined ? undefined : true} data-testid={props["data-testid"]} data-placeholder={selected === undefined ? "true" : undefined}
        onClick={() => (state.open ? close(false) : open())} onKeyDown={onTriggerKey}>
        <span className="ui-picker-value">{selected === undefined ? props.placeholder ?? "Choose…" : selected.label}</span>
        {selected?.badge === undefined ? null : <span className="ui-picker-tag" data-tone={selected.badgeTone ?? "neutral"}>{selected.badge}</span>}
        <Chevron />
      </button>
      {state.open ? (
        <PickerList id={id} label={label} shown={shown} active={state.active} value={value} searchable={searchable} query={state.query}
          emptyText={props.emptyText ?? "Nothing matches"} onQuery={(query) => setState({ open: true, query, active: moveIndex(filterOptions(options, query), -1, 1) })}
          onKeyDown={onListKey} onHover={move} onChoose={choose} />
      ) : null}
      {hint === undefined ? null : <p className="ui-field-hint" id={hintId}>{hint}</p>}
      {error === undefined ? null : <p className="ui-field-error" id={errorId}>{error}</p>}
    </div>
  );
}

interface ListKeyContext {
  readonly shown: readonly PickerOption[];
  readonly active: number;
  readonly searchable: boolean;
  readonly move: (index: number) => void;
  readonly choose: (option: PickerOption) => void;
  readonly close: (refocus: boolean) => void;
  readonly typeKey: (key: string) => void;
}

/** The open list's keys; true when the key was handled and its default must not run. */
export function listKey(event: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey">, context: ListKeyContext): boolean {
  const { shown, active } = context;
  const moves: Record<string, number> = {
    ArrowDown: moveIndex(shown, active, 1), ArrowUp: moveIndex(shown, active, -1), Home: moveIndex(shown, -1, 1), End: moveIndex(shown, shown.length, -1),
  };
  const to = moves[event.key];
  if (to !== undefined) {
    context.move(to);
    return true;
  }
  if (event.key === "Enter") {
    const option = shown[active];
    if (option !== undefined) context.choose(option);
    return true;
  }
  if (event.key === "Escape") {
    context.close(true);
    return true;
  }
  if (event.key === "Tab") {
    context.close(false);
    return false;
  }
  if (!context.searchable && event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
    context.typeKey(event.key);
    return true;
  }
  return false;
}

interface PickerListProps {
  readonly id: string;
  readonly label: string;
  readonly shown: readonly PickerOption[];
  readonly active: number;
  readonly value: string | null;
  readonly searchable: boolean;
  readonly query: string;
  readonly emptyText: string;
  readonly onQuery: (query: string) => void;
  readonly onKeyDown: (event: KeyboardEvent) => void;
  readonly onHover: (index: number) => void;
  readonly onChoose: (option: PickerOption) => void;
}

function PickerList(props: PickerListProps) {
  const { id, shown, active, searchable } = props;
  const focusTarget = useRef<HTMLElement>(null);
  const activeId = active >= 0 && shown[active] !== undefined ? `${id}-option-${active}` : undefined;
  useLayoutEffect(() => { focusTarget.current?.focus(); }, []);
  useEffect(() => {
    const node = activeId === undefined ? null : document.getElementById(activeId);
    if (node !== null && typeof node.scrollIntoView === "function") node.scrollIntoView({ block: "nearest" });
  }, [activeId]);
  const groups = groupOptions(shown);
  let index = -1;
  const option = (item: PickerOption): ReactNode => {
    index += 1;
    const at = index;
    return (
      <li key={item.id} id={`${id}-option-${at}`} role="option" className="ui-picker-option" aria-selected={item.id === props.value} aria-disabled={item.disabled || undefined}
        data-active={at === active ? "true" : undefined} onPointerMove={() => { if (at !== active) props.onHover(at); }}
        onClick={() => props.onChoose(item)}>
        <span className="ui-picker-option-main">
          <span className="ui-picker-option-label">{item.label}</span>
          {item.badge === undefined ? null : <span className="ui-picker-tag" data-tone={item.badgeTone ?? "neutral"}>{item.badge}</span>}
        </span>
        {item.description === undefined ? null : <span className="ui-picker-option-description">{item.description}</span>}
        <Check selected={item.id === props.value} />
      </li>
    );
  };
  return (
    <div className="ui-picker-pop" onKeyDown={props.onKeyDown}>
      {searchable ? (
        <input ref={focusTarget as React.RefObject<HTMLInputElement>} className="ui-picker-search" type="text" role="searchbox" placeholder="Search" aria-label={`Search ${props.label}`}
          aria-controls={`${id}-list`} aria-activedescendant={activeId} autoComplete="off" spellCheck={false} value={props.query} onChange={(event) => props.onQuery(event.target.value)} />
      ) : null}
      <ul ref={searchable ? undefined : focusTarget as React.RefObject<HTMLUListElement>} className="ui-picker-list" id={`${id}-list`} role="listbox" aria-labelledby={`${id}-label`}
        tabIndex={searchable ? -1 : 0} aria-activedescendant={searchable ? undefined : activeId}>
        {shown.length === 0 ? <li className="ui-picker-empty" role="presentation">{props.emptyText}</li> : null}
        {groups.map((group) => group.label === null ? group.options.map(option) : (
          <li key={group.label} role="presentation" className="ui-picker-group">
            <span className="ui-picker-group-label" id={`${id}-group-${group.label}`}>{group.label}</span>
            <ul role="group" aria-labelledby={`${id}-group-${group.label}`}>{group.options.map(option)}</ul>
          </li>
        ))}
      </ul>
    </div>
  );
}

export function Chevron() {
  return (
    <svg className="ui-picker-chevron" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
      <path d="M4 6l4 4 4-4" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function Check({ selected }: { readonly selected: boolean }) {
  if (!selected) return null;
  return (
    <svg className="ui-picker-check" viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
      <path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
