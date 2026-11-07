// The chat header's view controls: a segmented picker of the modes that apply to this agent (arrow
// keys move and choose, as a radio group does) and the Details switch, the TUI's Ctrl+T.

import type { aether } from "@aether-ui/protocol";
import { useRef, type KeyboardEvent } from "react";
import { invariant } from "../fleet/invariant.ts";
import { modeLabel } from "./modes.ts";
import type { ViewModeState } from "./useViewMode.ts";

const STEP: Readonly<Record<string, number>> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };

function ModePicker({ modes, mode, choose }: { readonly modes: readonly aether.ViewMode[]; readonly mode: string; readonly choose: (name: string) => void }) {
  const group = useRef<HTMLDivElement>(null);
  const current = Math.max(0, modes.findIndex((m) => m.name === mode));
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = STEP[event.key] ?? (event.key === "Home" ? -current : event.key === "End" ? modes.length - 1 - current : undefined);
    if (step === undefined) return;
    event.preventDefault();
    const next = modes[(current + step + modes.length) % modes.length];
    invariant(next !== undefined, "the picker moves within its modes");
    choose(next.name);
    group.current?.querySelector<HTMLElement>(`[data-mode="${next.name}"]`)?.focus();
  };
  return (
    <div ref={group} className="conv-modes" role="radiogroup" aria-label="View mode" onKeyDown={onKeyDown}>
      {modes.map((m) => {
        const checked = m.name === mode;
        return (
          <button key={m.name} type="button" role="radio" className="conv-mode" data-mode={m.name} aria-checked={checked} tabIndex={checked ? 0 : -1}
            title={m.description ?? undefined} onClick={() => choose(m.name)}>
            {modeLabel(m)}
          </button>
        );
      })}
    </div>
  );
}

export function ModeBar({ view }: { readonly view: ViewModeState }) {
  const hides = view.mode !== "standard";
  return (
    <div className="conv-bar">
      {view.modes.length > 1 ? <ModePicker modes={view.modes} mode={view.mode} choose={view.choose} /> : null}
      {hides ? (
        <button type="button" className="conv-details" aria-pressed={view.details} aria-keyshortcuts="Control+T"
          title="Show what this mode hides: tools, reasoning, memory and usage (Ctrl+T)" onClick={() => view.setDetails(!view.details)}>
          Details <kbd className="conv-kbd">Ctrl T</kbd>
        </button>
      ) : null}
      {view.problem === null ? null : <span className="conv-note" role="status">{view.problem}</span>}
      {view.unsaved === null ? null : <span className="conv-note" role="status">{view.unsaved}</span>}
    </div>
  );
}
