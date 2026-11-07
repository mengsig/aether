import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { invariant } from "./classes.ts";
import { Dialog } from "./dialog.tsx";

export interface Command {
  readonly id: string;
  readonly label: string;
  /** Shown beside the label and matched by the filter, e.g. "Go to" or "Theme". */
  readonly group: string;
  /** A status line under the label, matched by the filter. */
  readonly subtitle?: string;
  /** Extra words the filter matches but the palette does not show: ids, roles, repositories. */
  readonly keywords?: readonly string[];
  /** An action rather than a place: running it never adds it to the recent destinations. */
  readonly transient?: boolean;
  readonly run: () => void;
}

/** The most options the palette renders; the rest are reached by typing more. */
export const MAX_SHOWN = 50;
export const MAX_RECENTS = 8;

interface IndexedCommand {
  readonly command: Command;
  readonly label: string;
  readonly text: string;
}

/** Commands with their lowercased text computed once, so each keystroke only scans. */
export interface CommandIndex {
  readonly entries: readonly IndexedCommand[];
  readonly byId: ReadonlyMap<string, Command>;
}

export function indexCommands(commands: readonly Command[]): CommandIndex {
  const byId = new Map<string, Command>();
  const entries = commands.map((command) => {
    invariant(!byId.has(command.id), `command ids are unique, but ${command.id} repeats`);
    byId.set(command.id, command);
    const text = [command.label, command.group, command.subtitle ?? "", ...(command.keywords ?? [])].join(" ").toLowerCase();
    return { command, label: command.label.toLowerCase(), text };
  });
  return { entries, byId };
}

export interface Matches {
  readonly shown: readonly Command[];
  /** Every command that matched, including those past the cap. */
  readonly total: number;
}

const WHOLE_QUERY_PREFIX = 0;
const FIRST_WORD_PREFIX = 1;
const FIRST_WORD_IN_LABEL = 2;
const OTHER_TEXT = 3;

/** Commands whose text holds every typed word; label matches first, then index order. One pass, no sort. */
export function searchIndex(index: CommandIndex, query: string, limit: number = MAX_SHOWN): Matches {
  invariant(limit > 0, "the result cap is positive");
  const words = query.toLowerCase().split(/\s+/).filter((word) => word.length > 0);
  const first = words[0];
  if (first === undefined) return { shown: index.entries.slice(0, limit).map((entry) => entry.command), total: index.entries.length };
  const whole = words.join(" ");
  const tiers: Command[][] = [[], [], [], []];
  let total = 0;
  for (const entry of index.entries) {
    if (!words.every((word) => entry.text.includes(word))) continue;
    total += 1;
    const tier = entry.label.startsWith(whole) ? WHOLE_QUERY_PREFIX
      : entry.label.startsWith(first) ? FIRST_WORD_PREFIX
        : entry.label.includes(first) ? FIRST_WORD_IN_LABEL : OTHER_TEXT;
    tiers[tier]?.push(entry.command);
  }
  return { shown: tiers.flat().slice(0, limit), total };
}

/** The commands matching `query`, best first, uncapped; the palette itself goes through `searchIndex`. */
export function filterCommands(commands: readonly Command[], query: string): Command[] {
  return [...searchIndex(indexCommands(commands), query, Math.max(commands.length, 1)).shown];
}

/** With nothing typed: recent destinations that still exist, then everything else in order. */
export function browseCommands(index: CommandIndex, recents: readonly string[], limit: number = MAX_SHOWN): Matches {
  const recent = recents.flatMap((id) => index.byId.get(id) ?? []);
  const recentIds = new Set(recent.map((command) => command.id));
  const rest = index.entries.filter((entry) => !recentIds.has(entry.command.id)).map((entry) => entry.command);
  invariant(recent.length + rest.length === index.entries.length, "recents and the rest cover every command once");
  return { shown: [...recent, ...rest].slice(0, limit), total: index.entries.length };
}

/** `id` first, the rest as before without it, at most `max` long. */
export function pushRecent(recents: readonly string[], id: string, max: number = MAX_RECENTS): string[] {
  invariant(id.length > 0, "a recent destination has an id");
  return [id, ...recents.filter((held) => held !== id)].slice(0, max);
}

export interface CommandPaletteProps {
  readonly open: boolean;
  readonly onClose: () => void;
  readonly commands: readonly Command[];
  /** Ids of the commands last run, newest first; shown first while the query is empty. */
  readonly recents?: readonly string[];
  /** Called after the palette closes and before the command runs. */
  readonly onRun?: (command: Command) => void;
}

/** A modal combobox over every command; arrows move, Enter runs, Escape closes. */
export function CommandPalette({ open, onClose, commands, recents = [], onRun }: CommandPaletteProps) {
  return (
    <Dialog open={open} onClose={onClose} title="Go anywhere" className="ui-palette">
      {open ? <PaletteSearch commands={commands} recents={recents} onRun={onRun} onClose={onClose} /> : null}
    </Dialog>
  );
}

interface SearchProps extends Pick<CommandPaletteProps, "commands" | "onClose"> {
  readonly recents: readonly string[];
  readonly onRun: CommandPaletteProps["onRun"] | undefined;
}

function PaletteSearch({ commands, recents, onRun, onClose }: SearchProps) {
  const id = useId();
  const [query, setQuery] = useState("");
  const [cursor, setCursor] = useState(0);
  const index = useMemo(() => indexCommands(commands), [commands]);
  const matches = useMemo(
    () => (query.trim() === "" ? browseCommands(index, recents) : searchIndex(index, query)),
    [index, query, recents],
  );
  const { shown } = matches;
  const recentCount = query.trim() === "" ? recents.filter((recent) => index.byId.has(recent)).length : 0;
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
    onRun?.(command);
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
      <input className="ui-palette-input" type="search" role="combobox" aria-label="Search screens, agents and work items" aria-expanded="true"
        aria-controls={`${id}-list`} aria-autocomplete="list" aria-activedescendant={active < 0 ? undefined : `${id}-${active}`}
        placeholder="Jump to a screen, agent, item or pull request" value={query} data-autofocus
        onChange={(event) => { setQuery(event.currentTarget.value); setCursor(0); }} onKeyDown={onKeyDown} />
      <ul ref={list} className="ui-palette-list" role="listbox" id={`${id}-list`} aria-label="Matching commands">
        {shown.map((command, position) => (
          <li key={command.id} id={`${id}-${position}`} role="option" aria-selected={position === active} className="ui-palette-option"
            onPointerMove={() => setCursor(position)} onClick={() => run(command)}>
            <span className="ui-palette-label">{command.label}</span>
            <span className="ui-palette-group">{position < recentCount ? `Recent · ${command.group}` : command.group}</span>
            {command.subtitle ? <span className="ui-palette-subtitle">{command.subtitle}</span> : null}
          </li>
        ))}
      </ul>
      {shown.length === 0 ? <p className="ui-palette-empty" role="status">Nothing matches “{query}”.</p> : null}
      {matches.total > shown.length
        ? <p className="ui-palette-more" role="status">Showing {shown.length} of {matches.total}. Keep typing to narrow it.</p> : null}
    </div>
  );
}
