import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { invariant } from "./classes.ts";
import { Dialog } from "./dialog.tsx";

export interface Command {
  readonly id: string;
  readonly label: string;
  /** Shown beside the label and matched by the filter, e.g. "Go to" or "Theme". */
  readonly group: string;
  readonly run: () => void;
}

/** Commands whose label and group hold every typed word, label-prefix matches first. */
export function filterCommands(commands: readonly Command[], query: string): Command[] {
  const words = query.toLowerCase().split(/\s+/).filter((word) => word.length > 0);
  const matches = commands.filter((command) => {
    const text = `${command.label} ${command.group}`.toLowerCase();
    return words.every((word) => text.includes(word));
  });
  const first = words[0];
  if (first === undefined) return matches;
  const prefixed = (command: Command) => (command.label.toLowerCase().startsWith(first) ? 0 : 1);
  return matches.map((command, order) => ({ command, order }))
    .sort((a, b) => prefixed(a.command) - prefixed(b.command) || a.order - b.order)
    .map((entry) => entry.command);
}

export interface CommandPaletteProps {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly commands: readonly Command[];
}

/** A modal combobox over every command; arrows move, Enter runs, Escape closes. */
export function CommandPalette({ open, onClose, commands }: CommandPaletteProps) {
  invariant(new Set(commands.map((command) => command.id)).size === commands.length, "command ids are unique");
  return (
    <Dialog open={open} onClose={onClose} title="Go anywhere" className="ui-palette">
      {open ? <PaletteSearch commands={commands} onClose={onClose} /> : null}
    </Dialog>
  );
}

function PaletteSearch({ commands, onClose }: { readonly commands: readonly Command[]; readonly onClose: () => void }) {
  const id = useId();
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const shown = filterCommands(commands, query);
  const active = shown.length === 0 ? -1 : Math.min(cursor, shown.length - 1);
  const list = useRef<HTMLUListElement>(null);
  // Focus stays in the input, so the browser never scrolls the active option into view by itself.
  useEffect(() => {
    if (active < 0) return;
    const option = list.current?.children[active];
    invariant(option instanceof HTMLElement, "the active option is rendered in the list");
    option.scrollIntoView({ block: "nearest" });
  }, [active]);
  const run = (command: Command | undefined) => {
    if (command === undefined) return;
    onClose();
    command.run();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      if (shown.length === 0) return;
      setCursor((active + (event.key === "ArrowDown" ? 1 : -1) + shown.length) % shown.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      run(shown[active]);
    }
  };
  return (
    <div className="ui-palette-search">
      <input className="ui-palette-input" type="search" role="combobox" aria-label="Search commands and screens" aria-expanded="true"
        aria-controls={`${id}-list`} aria-autocomplete="list" aria-activedescendant={active < 0 ? undefined : `${id}-${active}`}
        placeholder="Type a screen or command" value={query} data-autofocus
        onChange={(event) => { setQuery(event.currentTarget.value); setCursor(0); }} onKeyDown={onKeyDown} />
      <ul ref={list} className="ui-palette-list" role="listbox" id={`${id}-list`} aria-label="Matching commands">
        {shown.map((command, index) => (
          <li key={command.id} id={`${id}-${index}`} role="option" aria-selected={index === active} className="ui-palette-option"
            onPointerMove={() => setCursor(index)} onClick={() => run(command)}>
            <span>{command.label}</span>
            <span className="ui-palette-group">{command.group}</span>
          </li>
        ))}
      </ul>
      {shown.length === 0 ? <p className="ui-palette-empty" role="status">Nothing matches “{query}”.</p> : null}
    </div>
  );
}
