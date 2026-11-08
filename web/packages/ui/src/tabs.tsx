import { useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode, type RefObject } from "react";
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

  const indicator = useIndicator(refs, current.id);
  return (
    <div className={classes("ui-tabs", className)}>
      <div className="ui-tablist" role="tablist" aria-label={label} onKeyDown={onKeyDown} data-measured={indicator === null ? undefined : "true"}>
        {tabs.map((tab) => (
          <button key={tab.id} type="button" role="tab" className="ui-tab" id={`${base}-tab-${tab.id}`}
            aria-selected={tab === current} aria-controls={`${base}-panel-${tab.id}`} tabIndex={tab === current ? 0 : -1}
            ref={(node) => { if (node) refs.current.set(tab.id, node); else refs.current.delete(tab.id); }}
            onClick={() => choose(tab.id)}>
            {tab.label}
          </button>
        ))}
        {indicator === null ? null : <span className="ui-tab-indicator" aria-hidden="true" style={indicator} />}
      </div>
      <div key={current.id} className="ui-tabpanel" role="tabpanel" id={`${base}-panel-${current.id}`} aria-labelledby={`${base}-tab-${current.id}`} tabIndex={0}>
        {current.panel}
      </div>
    </div>
  );
}

/** Where the one underline sits: under the selected tab, measured after layout and on resize; null until measured. */
function useIndicator(refs: RefObject<Map<string, HTMLButtonElement>>, selected: string): CSSProperties | null {
  const [box, setBox] = useState<{ readonly left: number; readonly width: number } | null>(null);
  useLayoutEffect(() => {
    const tab = refs.current.get(selected);
    invariant(tab !== undefined, `the selected tab ${selected} is mounted`);
    const measure = () => setBox(tab.offsetWidth > 0 ? { left: tab.offsetLeft, width: tab.offsetWidth } : null);
    measure();
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(tab);
    if (tab.parentElement !== null) observer.observe(tab.parentElement);
    return () => observer.disconnect();
  }, [refs, selected]);
  if (box === null) return null;
  invariant(Number.isFinite(box.left) && box.width > 0, "the tab indicator has a real box");
  return { inlineSize: `${box.width}px`, translate: `${box.left}px 0` };
}
