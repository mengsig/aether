// The view mode of one agent's chat: the daemon's catalog (`list_view_modes`, never read from files
// here), the choice remembered per daemon and agent, and the Details switch that reveals what it hides.

import { useReread } from "@aether-ui/core";
import { aether, type ConnId } from "@aether-ui/protocol";
import { useMemo, useState } from "react";
import { localStored } from "../composer/drafts.ts";
import { invariant } from "../fleet/invariant.ts";
import { applies, initialMode, type ModeRules } from "./modes.ts";

const PREFIX = "aether.view-mode.v1:";
const STANDARD = "standard";

export function modeKey(conn: ConnId, agent: string): string {
  invariant(conn !== "" && agent !== "", "a saved mode belongs to a connection and an agent");
  return PREFIX + JSON.stringify([conn, agent]);
}

/** The saved mode name, or null; storage that cannot be read, or holds anything else, gives none. */
export function readSavedMode(key: string): string | null {
  invariant(key.startsWith(PREFIX), "saved modes are read by a key from modeKey");
  const raw = localStored((store) => store.getItem(key));
  if ("problem" in raw) return null;
  return raw.value !== null && /^[a-z][a-z0-9_-]{0,47}$/.test(raw.value) ? raw.value : null;
}

function catalogOf(value: unknown): aether.ViewModes | null {
  if (value === null || typeof value !== "object") return null;
  const catalog = value as Partial<aether.ViewModes>;
  invariant(Array.isArray(catalog.modes) && Array.isArray(catalog.diagnostics), "a view_modes reply lists its modes and diagnostics");
  invariant(catalog.modes.some((m) => m.name === STANDARD), "the daemon's catalog has the standard mode");
  return catalog as aether.ViewModes;
}

/**
 * Why the catalog may lack a user's mode, or null when it is whole: the last read failed (an older
 * answer may still be shown), or a mode file could not be read or was rejected.
 */
function incompleteBecause(error: string | null, catalog: aether.ViewModes | null): string | null {
  if (error !== null) return `View modes unavailable: ${error}`;
  const first = catalog?.diagnostics[0];
  if (first === undefined) return null;
  const more = (catalog?.diagnostics.length ?? 1) - 1;
  return `Some view modes could not be read: ${first.source}: ${first.reason}${more > 0 ? ` (and ${more} more)` : ""}`;
}

export interface ViewModeState {
  /** The modes that apply to this agent, standard first; empty until the daemon answers. */
  readonly modes: readonly aether.ViewMode[];
  readonly mode: string;
  /** What to draw: null draws every row whole (standard, or Details on). */
  readonly rules: ModeRules | null;
  readonly details: boolean;
  readonly choose: (name: string) => void;
  /** `/focus` and the like: a mode toggles with standard. */
  readonly toggle: (name: string) => void;
  readonly setDetails: (on: boolean) => void;
  /** Why the catalog may be missing a mode (see `incompleteBecause`), or null when it is whole or still loading. */
  readonly problem: string | null;
  /** Why the chosen mode will not be remembered (the store refused it), or null. */
  readonly unsaved: string | null;
}

export function useViewMode(conn: ConnId, agent: aether.Agent): ViewModeState {
  const read = useReread(conn, "view-modes", aether.listViewModes(), ["view_modes"]);
  const catalog = catalogOf(read?.value ?? null);
  const key = modeKey(conn, agent.id);
  const [chosen, setChosen] = useState<{ key: string; name: string } | null>(null);
  const [unsaved, setUnsaved] = useState<string | null>(null);
  const [details, setDetails] = useState(false);
  const modes = useMemo(() => {
    const usable = (catalog?.modes ?? []).filter((m) => applies(m, agent));
    return [...usable.filter((m) => m.name === STANDARD), ...usable.filter((m) => m.name !== STANDARD)];
  }, [catalog, agent]);
  const mode = catalog === null ? STANDARD : initialMode(modes, agent, chosen?.key === key ? chosen.name : readSavedMode(key));
  const choose = (name: string) => {
    invariant(modes.some((m) => m.name === name), `${name} is a mode this agent can use`);
    const saved = localStored((store) => store.setItem(key, name));
    setUnsaved("problem" in saved ? `This view is not remembered after a reload (${saved.problem})` : null);
    setChosen({ key, name });
  };
  const toggle = (name: string) => choose(mode === name ? STANDARD : name);
  const definition = modes.find((m) => m.name === mode);
  const rules = details || definition === undefined || mode === STANDARD ? null : definition;
  const problem = incompleteBecause(read?.error ?? null, catalog);
  return { modes, mode, rules, details, choose, toggle, setDetails, problem, unsaved };
}
