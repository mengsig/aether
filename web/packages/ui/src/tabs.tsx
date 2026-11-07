import { useId, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { classes, invariant } from "./classes.ts";

export interface TabItem {
  readonly id: string;
  readonly label: string;
  readonly panel: ReactNode;
}

export interface TabsProps {
  readonly label: string;
  readonly tabs: readonly TabItem[];
  /** Controls the selection; without it the tabs keep their own, starting at the first. */
  readonly selected?: string;
  readonly onSelect?: (id: string) => void;
  readonly className?: string;
}

/** Tabs with automatic activation and a roving tabindex (WAI-ARIA APG). */
export function Tabs({ label, tabs, selected, onSelect, className }: TabsProps) {
  invariant(tabs.length > 0, `tabs ${label} have no tabs`);
  invariant(new Set(tabs.map((tab) => tab.id)).size === tabs.length, `tabs ${label} repeat an id`);
  const [own, setOwn] = useState(tabs[0]?.id ?? "");
  const current = tabs.find((tab) => tab.id === (selected ?? own)) ?? tabs[0];
  invariant(current !== undefined, "a tab is selected");
  const base = useId();
  const refs = useRef(new Map<string, HTMLButtonElement>());

  const choose = (id: string) => {
    setOwn(id);
    onSelect?.(id);
    refs.current.get(id)?.focus();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    const at = tabs.indexOf(current);
    const targets: Record<string, number> = { ArrowRight: at + 1, ArrowLeft: at - 1, Home: 0, End: tabs.length - 1 };
    const target = targets[event.key];
    if (target === undefined) return;
    event.preventDefault();
    const tab = tabs[(target + tabs.length) % tabs.length];
    if (tab) choose(tab.id);
  };

  return (
    <div className={classes("ui-tabs", className)}>
      <div className="ui-tablist" role="tablist" aria-label={label} onKeyDown={onKeyDown}>
        {tabs.map((tab) => (
          <button key={tab.id} type="button" role="tab" className="ui-tab" id={`${base}-tab-${tab.id}`}
            aria-selected={tab === current} aria-controls={`${base}-panel-${tab.id}`} tabIndex={tab === current ? 0 : -1}
            ref={(node) => { if (node) refs.current.set(tab.id, node); else refs.current.delete(tab.id); }}
            onClick={() => choose(tab.id)}>
            {tab.label}
          </button>
        ))}
      </div>
      <div key={current.id} className="ui-tabpanel" role="tabpanel" id={`${base}-panel-${current.id}`} aria-labelledby={`${base}-tab-${current.id}`} tabIndex={0}>
        {current.panel}
      </div>
    </div>
  );
}
