// The view mode of one agent's chat: the daemon's catalog (`list_view_modes`, never read from files
// here), the choice remembered per daemon and agent, and the Details switch that reveals what it hides.

import { useReread } from "@aether-ui/core";
import { aether, type ConnId } from "@aether-ui/protocol";
import { useMemo, useState } from "react";
import { invariant } from "../fleet/invariant.ts";
import { applies, initialMode, type ModeRules } from "./modes.ts";

const PREFIX = "aether.view-mode.v1:";
const STANDARD = "standard";

export function modeKey(conn: ConnId, agent: string): string {
  invariant(conn !== "" && agent !== "", "a saved mode belongs to a connection and an agent");
  return PREFIX + JSON.stringify([conn, agent]);
}

/** The saved mode name, or null; storage holding anything else is not ours and is ignored. */
export function readSavedMode(key: string): string | null {
  invariant(key.startsWith(PREFIX), "saved modes are read by a key from modeKey");
  const raw = localStorage.getItem(key);
  return raw !== null && /^[a-z][a-z0-9_-]{0,47}$/.test(raw) ? raw : null;
}

function catalogOf(value: unknown): readonly aether.ViewMode[] | null {
  if (value === null || typeof value !== "object") return null;
  const modes = (value as Partial<aether.ViewModes>).modes;
  invariant(Array.isArray(modes), "a view_modes reply lists its modes");
  invariant(modes.some((m) => m.name === STANDARD), "the daemon's catalog has the standard mode");
  return modes;
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
  readonly problem: string | null;
}

export function useViewMode(conn: ConnId, agent: aether.Agent): ViewModeState {
  const read = useReread(conn, "view-modes", aether.listViewModes(), ["view_modes"]);
  const catalog = catalogOf(read?.value ?? null);
  const key = modeKey(conn, agent.id);
  const [chosen, setChosen] = useState<{ key: string; name: string } | null>(null);
  const [details, setDetails] = useState(false);
  const modes = useMemo(() => {
    const usable = (catalog ?? []).filter((m) => applies(m, agent));
    return [...usable.filter((m) => m.name === STANDARD), ...usable.filter((m) => m.name !== STANDARD)];
  }, [catalog, agent]);
  const mode = catalog === null ? STANDARD : initialMode(modes, agent, chosen?.key === key ? chosen.name : readSavedMode(key));
  const choose = (name: string) => {
    invariant(modes.some((m) => m.name === name), `${name} is a mode this agent can use`);
    localStorage.setItem(key, name);
    setChosen({ key, name });
  };
  const toggle = (name: string) => choose(mode === name ? STANDARD : name);
  const definition = modes.find((m) => m.name === mode);
  const rules = details || definition === undefined || mode === STANDARD ? null : definition;
  return { modes, mode, rules, details, choose, toggle, setDetails, problem: read?.error ?? null };
}
