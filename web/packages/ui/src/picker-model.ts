// The pure side of the pickers: options, groups, filtering and keyboard movement, with no DOM.

import { invariant } from "./classes.ts";

export interface PickerOption {
  /** The value the form receives; what the protocol sees is never changed by the picker. */
  readonly id: string;
  readonly label: string;
  readonly description?: string;
  /** Options with the same group are listed together under that heading, in first-seen order. */
  readonly group?: string;
  /** A short tag after the label: "Default", "Subscription", "Not signed in". */
  readonly badge?: string;
  readonly badgeTone?: "neutral" | "accent" | "warning" | "danger";
  readonly disabled?: boolean;
  /** Extra words a search matches, such as aliases or a provider id. */
  readonly keywords?: string;
}

export interface PickerGroup { readonly label: string | null; readonly options: readonly PickerOption[] }

/** Options grouped under their headings, groups in the order their first option appears. */
export function groupOptions(options: readonly PickerOption[]): PickerGroup[] {
  invariant(new Set(options.map((option) => option.id)).size === options.length, "picker options repeat an id");
  const groups = new Map<string | null, PickerOption[]>();
  for (const option of options) {
    const key = option.group ?? null;
    const members = groups.get(key);
    if (members === undefined) groups.set(key, [option]);
    else members.push(option);
  }
  return [...groups.entries()].map(([label, members]) => ({ label, options: members }));
}

function normalize(text: string): string {
  return text.toLocaleLowerCase().normalize("NFKD").replace(/[̀-ͯ]/g, "");
}

/** Options matching every word of `query` in their label, description, group or keywords. */
export function filterOptions(options: readonly PickerOption[], query: string): PickerOption[] {
  const words = normalize(query).split(/\s+/).filter((word) => word.length > 0);
  if (words.length === 0) return [...options];
  return options.filter((option) => {
    const haystack = normalize([option.label, option.description ?? "", option.group ?? "", option.keywords ?? "", option.id].join(" "));
    return words.every((word) => haystack.includes(word));
  });
}

/** The index `step` away from `from` among enabled options, wrapping; -1 when none is enabled. */
export function moveIndex(options: readonly PickerOption[], from: number, step: 1 | -1): number {
  invariant(from >= -1 && from <= options.length, `picker index ${from} is out of range`);
  if (options.length === 0) return -1;
  for (let offset = 1; offset <= options.length; offset += 1) {
    const index = (((from + step * offset) % options.length) + options.length) % options.length;
    if (!options[index]?.disabled) return index;
  }
  return -1;
}

/** Type-ahead: the first enabled option after `from` whose label starts with `typed`, wrapping.
    A longer prefix may keep the option at `from`; with nothing active (-1) it searches from the top. */
export function typeAhead(options: readonly PickerOption[], from: number, typed: string): number {
  invariant(from >= -1 && from < Math.max(options.length, 1), `type-ahead index ${from} is out of range`);
  const prefix = normalize(typed);
  if (prefix.length === 0 || options.length === 0) return -1;
  const start = prefix.length === 1 ? from : Math.max(from, 0) - 1;
  for (let offset = 1; offset <= options.length; offset += 1) {
    const index = (((start + offset) % options.length) + options.length) % options.length;
    const option = options[index];
    if (option !== undefined && !option.disabled && normalize(option.label).startsWith(prefix)) return index;
  }
  return -1;
}
