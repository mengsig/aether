// The on-demand Archon reads behind the C2 screens: item history, initiative detail and filtered item
// searches. Each reply is stored whole, stamped, and never replaces a newer one (store.ts `replaces`).

import { array, invariant, object, seq, string, type Json } from "./guards.ts";
import { replyData, replyObject } from "./replies.ts";
import { replaces, type Stamped } from "./store.ts";

/** The `list_items` filters of protocol 24 (CONTRACTS.md §1.3), each optional. */
export interface ItemSearch {
  readonly text?: string;
  /** RFC 3339 times: items updated at or after `updated_since`, and before `before`. */
  readonly updated_since?: string;
  readonly before?: string;
  readonly limit?: number;
}

/** Searches the store keeps; an older one is read again when asked for. */
export const SEARCHES_KEPT = 8;
export const HISTORY_LIMIT = 1000;
const TEXT_MAX = 200;

/** Checks a search against the protocol's bounds; a caller validates user input before it gets here. */
export function checkSearch(query: ItemSearch): void {
  if (query.text !== undefined) {
    const length = [...query.text].length;
    invariant(length >= 1 && length <= TEXT_MAX, `search text is 1 to ${TEXT_MAX} characters, not ${length}`);
  }
  for (const at of [query.updated_since, query.before]) invariant(at === undefined || Number.isFinite(Date.parse(at)), `${String(at)} is a time`);
  if (query.updated_since !== undefined && query.before !== undefined) {
    invariant(Date.parse(query.updated_since) < Date.parse(query.before), "a search's range starts before it ends");
  }
  invariant(query.limit === undefined || (Number.isInteger(query.limit) && query.limit >= 1), "a search limit is a positive integer");
}

/** One key per distinct search, whatever order its fields were written in. */
export function searchKey(query: ItemSearch): string {
  checkSearch(query);
  const fields = (["before", "limit", "text", "updated_since"] as const).filter((name) => query[name] !== undefined);
  return JSON.stringify(fields.map((name) => [name, query[name]]));
}

export function searchCommand(query: ItemSearch): Json {
  checkSearch(query);
  return { cmd: "list_items", ...query };
}

/** A `list_items` reply as stored: `{items}`, each item an object with an id. */
export function searchValue(reply: Json): Json {
  const items = array(replyData(reply, "items"), "list_items reply").map((row) => object(row, "listed item"));
  for (const row of items) string(row.id, "listed item id");
  return { items };
}

/** An `item_history` reply for `item`, its transitions in ascending seq. */
export function historyValue(reply: Json, item: string): Json {
  const value = replyObject(reply, "item_history");
  invariant(value.item === undefined || value.item === item, `the history of ${item} describes ${String(value.item)}`);
  const transitions = array(value.transitions, "item_history.transitions").map((row) => object(row, "transition"));
  let last = -1;
  for (const row of transitions) {
    const at = seq(row.seq, "transition.seq");
    if (at <= last) throw new TypeError(`item_history of ${item} is not in ascending seq order`);
    last = at;
    string(row.to, "transition.to");
  }
  return { item, transitions };
}

/** A `get_initiative` reply for `id`. */
export function initiativeValue(reply: Json, id: string): Json {
  const value = replyObject(reply, "initiative");
  const initiative = object(value.initiative, "initiative_detail.initiative");
  invariant(initiative.id === id, `the detail of initiative ${id} describes ${String(initiative.id)}`);
  array(value.members, "initiative_detail.members");
  return value;
}

/**
 * `record` with `value` at `key` when it is at least as new as the one held, else `record` itself. With
 * `keep`, the oldest entries beyond it are dropped, so a store of searches stays bounded.
 */
export function withStamped(record: Readonly<Record<string, Stamped>>, key: string, value: Stamped, keep?: number): Readonly<Record<string, Stamped>> {
  invariant(key.length > 0, "a stamped entry has a key");
  const held = record[key];
  if (held !== undefined && !replaces(value.asOf, held.asOf)) return record;
  const entries = Object.entries(record).filter(([name]) => name !== key);
  entries.push([key, value]);
  const kept = keep === undefined ? entries : entries.slice(-keep);
  invariant(keep === undefined || kept.length <= keep, "a bounded record keeps at most its bound");
  return Object.fromEntries(kept);
}
