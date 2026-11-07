// The `/` menu: which options a draft offers, the highlighted one, and the listbox that shows them.
// The textarea keeps focus throughout; it owns the combobox role and points at the active option.

import { useEffect, useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { matchingChoices, matchingCommands, needsArgument, slashQuery, type SlashChoice, type SlashCommand } from "./slash.ts";

export type MenuOption =
  | { readonly kind: "command"; readonly command: SlashCommand }
  | { readonly kind: "choice"; readonly command: SlashCommand; readonly choice: SlashChoice };

/** What picking an option does: put text in the draft, or run the line. */
export type Pick = { readonly kind: "complete"; readonly text: string } | { readonly kind: "run"; readonly line: string };

export function optionsFor(draft: string, commands: readonly SlashCommand[]): readonly MenuOption[] {
  const query = slashQuery(draft, commands);
  if (query.kind === "none") return [];
  if (query.kind === "argument") return matchingChoices(query.command, query.prefix).map((choice) => ({ kind: "choice", command: query.command, choice }));
  return matchingCommands(commands, query.prefix).map((command) => ({ kind: "command", command }));
}

/** Tab: the option's full text, so the person can go on typing its argument. */
export function completion(option: MenuOption): string {
  if (option.kind === "choice") return `/${option.command.name} ${option.choice.value}`;
  return option.command.args === "" ? `/${option.command.name}` : `/${option.command.name} `;
}

/** Enter or a click: a command that must have an argument is completed, anything else runs. */
export function pick(option: MenuOption): Pick {
  if (option.kind === "command" && needsArgument(option.command)) return { kind: "complete", text: completion(option) };
  return { kind: "run", line: completion(option).trimEnd() };
}

export interface MenuState {
  readonly options: readonly MenuOption[];
  readonly open: boolean;
  readonly active: number;
  readonly move: (delta: number) => void;
  readonly hover: (index: number) => void;
  readonly dismiss: () => void;
}

/** The highlight resets to the first option whenever the draft changes; Escape closes it for this draft. */
export function useMenu(draft: string, commands: readonly SlashCommand[]): MenuState {
  const options = optionsFor(draft, commands);
  const [highlight, setHighlight] = useState({ draft, index: 0 });
  const [dismissed, setDismissed] = useState<string | null>(null);
  const index = highlight.draft === draft ? highlight.index : 0;
  const active = Math.min(index, Math.max(options.length - 1, 0));
  invariant(options.length === 0 || (active >= 0 && active < options.length), "the highlight is one of the options");
  const move = (delta: number) => {
    invariant(delta === 1 || delta === -1, "the highlight moves one option at a time");
    setHighlight({ draft, index: (active + delta + options.length) % Math.max(options.length, 1) });
  };
  return {
    options, active, move,
    open: options.length > 0 && dismissed !== draft,
    hover: (next) => setHighlight({ draft, index: next }),
    dismiss: () => setDismissed(draft),
  };
}

export const optionId = (listbox: string, index: number): string => `${listbox}-option-${index}`;

interface Group { readonly name: string; readonly start: number; readonly options: readonly MenuOption[] }

/** Options in runs of one group each, keeping their order and each one's index in the flat list. */
function groupsOf(options: readonly MenuOption[]): readonly Group[] {
  const groups: { name: string; start: number; options: MenuOption[] }[] = [];
  options.forEach((option, index) => {
    const name = option.kind === "choice" ? `/${option.command.name} ${option.command.args}` : option.command.group;
    const last = groups.at(-1);
    if (last?.name === name) last.options.push(option);
    else groups.push({ name, start: index, options: [option] });
  });
  invariant(groups.reduce((n, g) => n + g.options.length, 0) === options.length, "every option is in one group");
  return groups;
}

export interface SlashMenuProps {
  readonly id: string;
  readonly menu: MenuState;
  readonly onPick: (option: MenuOption) => void;
}

export function SlashMenu({ id, menu, onPick }: SlashMenuProps) {
  invariant(menu.open, "only an open menu is shown");
  useEffect(() => {
    document.getElementById(optionId(id, menu.active))?.scrollIntoView?.({ block: "nearest" });
  }, [id, menu.active]);
  return (
    <div className="cmp-menu" id={id} role="listbox" aria-label="Commands">
      {groupsOf(menu.options).map((group) => (
        <div key={group.name} role="group" aria-labelledby={`${id}-group-${group.start}`} className="cmp-group">
          <div id={`${id}-group-${group.start}`} className="cmp-group-name" role="presentation">{group.name}</div>
          {group.options.map((option, offset) => {
            const index = group.start + offset;
            const label = option.kind === "choice" ? option.choice.value : `/${option.command.name}`;
            const hint = option.kind === "choice" ? "" : option.command.args;
            const about = option.kind === "choice" ? option.choice.about ?? option.choice.label : option.command.about;
            return (
              <div key={label} id={optionId(id, index)} role="option" aria-selected={index === menu.active} className="cmp-option"
                onMouseMove={() => { if (index !== menu.active) menu.hover(index); }}
                onMouseDown={(event) => event.preventDefault()} onClick={() => onPick(option)}>
                <span className="cmp-option-name">{label}</span>
                {hint ? <span className="cmp-option-args">{hint}</span> : null}
                {about ? <span className="cmp-option-about">{about}</span> : null}
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}
