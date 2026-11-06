import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { Button, type ButtonVariant } from "./button.tsx";
import { classes, invariant } from "./classes.ts";

export interface MenuItem {
  readonly id: string;
  readonly label: string;
  readonly onSelect: () => void;
  readonly disabled?: boolean;
  readonly tone?: "danger";
}

export interface MenuProps {
  /** The trigger's text. */
  readonly label: string;
  readonly items: readonly MenuItem[];
  readonly variant?: ButtonVariant;
  readonly size?: "sm" | "md";
  readonly className?: string;
}

/** The index `step` places away from `from` among enabled items, wrapping around; -1 when none is enabled. */
export function nextEnabled(items: readonly MenuItem[], from: number, step: 1 | -1): number {
  invariant(from >= -1 && from <= items.length, `menu index ${from} is out of range`);
  for (let offset = 1; offset <= items.length; offset += 1) {
    const index = (((from + step * offset) % items.length) + items.length) % items.length;
    if (!items[index]?.disabled) return index;
  }
  return -1;
}

/** A menu button (WAI-ARIA APG): arrows move, Enter selects, Escape and Tab close it. */
export function Menu({ label, items, variant = "secondary", size = "md", className }: MenuProps) {
  invariant(items.length > 0, `menu ${label} has no items`);
  const id = useId();
  const [active, setActive] = useState(-1);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const open = active >= 0;

  useEffect(() => {
    if (open) itemRefs.current[active]?.focus();
  }, [open, active]);

  useEffect(() => {
    if (!open) return undefined;
    const outside = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setActive(-1);
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [open]);

  const close = (refocus: boolean) => {
    setActive(-1);
    if (refocus) trigger.current?.focus();
  };
  const select = (item: MenuItem) => {
    close(true);
    item.onSelect();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    const moves: Record<string, number> = {
      ArrowDown: nextEnabled(items, active, 1), ArrowUp: nextEnabled(items, active, -1),
      Home: nextEnabled(items, -1, 1), End: nextEnabled(items, items.length, -1),
    };
    const move = moves[event.key];
    if (move !== undefined) {
      event.preventDefault();
      setActive(move);
    } else if (event.key === "Escape") {
      event.preventDefault();
      close(true);
    } else if (event.key === "Tab") close(false);
  };
  const openAt = (index: number) => setActive(index);

  return (
    <div className={classes("ui-menu", className)} ref={root}>
      <Button ref={trigger} variant={variant} size={size} aria-haspopup="menu" aria-expanded={open} aria-controls={open ? id : undefined}
        onClick={() => (open ? close(false) : openAt(nextEnabled(items, -1, 1)))}
        onKeyDown={(event) => {
          if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
          event.preventDefault();
          openAt(event.key === "ArrowDown" ? nextEnabled(items, -1, 1) : nextEnabled(items, items.length, -1));
        }}>
        {label}
      </Button>
      {open ? (
        <div className="ui-menu-list" role="menu" id={id} aria-label={label} onKeyDown={onKeyDown}>
          {items.map((item, index) => (
            <button key={item.id} type="button" role="menuitem" className="ui-menu-item" tabIndex={-1} data-tone={item.tone}
              disabled={item.disabled} ref={(node) => { itemRefs.current[index] = node; }} onClick={() => select(item)}>
              {item.label}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
