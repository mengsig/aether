// Every key a settings patch changes, with the value the daemon holds now and the new one, so
// the confirmation shows the whole change. Keys come from the validated patch's known set.

import type { NoesisSettingsPatch } from "@aether-ui/protocol";
import type { SettingChange } from "../security/confirm.ts";

type Table = Readonly<Record<string, unknown>>;

function table(value: unknown, what: string): Table {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error(`${what} is not a table`);
  return value as Table;
}

function at(root: Table, path: readonly string[]): unknown {
  let value: unknown = root;
  for (const part of path) {
    if (typeof value !== "object" || value === null) return undefined;
    value = (value as Table)[part];
  }
  return value;
}

/** The path below `[noesis]` of a §6.1 key such as `noesis.budget.background_usd_daily`. */
function noesisPath(key: string): string[] {
  const parts = key.split(".");
  if (parts[0] !== "noesis" || parts.length < 2) throw new Error(`${key} is not a Noesis key`);
  return parts.slice(1);
}

/** Each patched key against `noesis_status`'s stored settings. */
export function noesisChanges(status: unknown, patch: NoesisSettingsPatch): SettingChange[] {
  const settings = table(table(status, "noesis_status").settings, "noesis_status settings");
  const entries = Object.entries(patch);
  if (entries.length === 0) throw new Error("A Noesis patch names at least one key");
  return entries.map(([key, value]) => ({ key, old: at(settings, noesisPath(key)), new: value }));
}

/** The flat §6.1 keys as the nested `NoesisSettingsPatch` the daemon reads. */
export function noesisWirePatch(patch: NoesisSettingsPatch): Record<string, unknown> {
  const wire: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(patch)) {
    const path = noesisPath(key);
    const last = path.pop();
    if (last === undefined) throw new Error(`${key} names a setting`);
    let node = wire;
    for (const part of path) node = (node[part] ??= {}) as Record<string, unknown>;
    node[last] = value;
  }
  if (Object.keys(wire).length === 0) throw new Error("the wire patch names at least one key");
  return wire;
}

// Archon settings that are tables: autonomy knobs show one line each; per-pipeline limits are
// replaced whole, so every pipeline old or new gets a line.
const KNOB_TABLES = new Set(["autonomy_default", "autonomy_max"]);
const ENTRY_TABLES = new Set(["max_per_pipeline"]);

function entryChanges(key: string, old: unknown, value: unknown): SettingChange[] {
  if (value === null) return [{ key, old, new: null }];
  const before = old === undefined || old === null ? {} : table(old, key);
  const after = table(value, key);
  const names = [...new Set([...Object.keys(before), ...Object.keys(after)])].sort();
  return names.map((entry) => ({ key, entry, old: before[entry], new: after[entry] }));
}

function storedOverlay(view: unknown): Table {
  return table(table(view, "settings").scoped, "the stored Archon settings");
}

/**
 * The whole global overlay `set_settings` stores: `set_settings` replaces it whole, so every
 * stored key the patch does not name is kept, and a knob table keeps its unnamed knobs.
 */
export function archonOverlay(view: unknown, patch: Table): Record<string, unknown> {
  const scoped = storedOverlay(view);
  const overlay: Record<string, unknown> = { ...scoped };
  for (const [key, value] of Object.entries(patch)) {
    const stored = scoped[key];
    const merge = KNOB_TABLES.has(key) && value !== null && stored !== undefined && stored !== null;
    overlay[key] = merge ? { ...table(stored, key), ...table(value, key) } : value;
  }
  if (!Object.keys(scoped).every((key) => key in overlay)) throw new Error("the overlay keeps every stored key");
  return overlay;
}

/** Each patched key against the global overlay `settings` returns (`scoped`). */
export function archonChanges(view: unknown, patch: Table): SettingChange[] {
  const scoped = storedOverlay(view);
  const changes: SettingChange[] = [];
  for (const [key, value] of Object.entries(patch)) {
    if (KNOB_TABLES.has(key)) {
      const knobs = table(value, key);
      for (const [knob, knobValue] of Object.entries(knobs)) {
        changes.push({ key: `${key}.${knob}`, old: at(scoped, [key, knob]), new: knobValue });
      }
    } else if (ENTRY_TABLES.has(key)) {
      changes.push(...entryChanges(key, scoped[key], value));
    } else {
      changes.push({ key, old: scoped[key], new: value });
    }
  }
  if (changes.length === 0) throw new Error("An Archon settings patch changes at least one key");
  return changes;
}
