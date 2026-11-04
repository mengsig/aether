import { cloneElement, useId, useState, type FocusEvent, type KeyboardEvent, type PointerEvent, type ReactElement } from "react";
import { invariant } from "./classes.ts";

interface TriggerProps {
  "aria-describedby"?: string;
  onFocus?: (event: FocusEvent) => void;
  onBlur?: (event: FocusEvent) => void;
  onPointerEnter?: (event: PointerEvent) => void;
  onPointerLeave?: (event: PointerEvent) => void;
  onKeyDown?: (event: KeyboardEvent) => void;
}

/** A description shown on hover and focus, dismissed with Escape (WCAG 1.4.13); the trigger stays focusable. */
export function Tooltip({ content, children }: { readonly content: string; readonly children: ReactElement<TriggerProps> }) {
  invariant(content.trim().length > 0, "a tooltip needs text");
  const id = useId();
  const [open, setOpen] = useState(false);
  const own = children.props;
  const describedBy = [own["aria-describedby"], id].filter(Boolean).join(" ");
  const trigger = cloneElement(children, {
    "aria-describedby": describedBy,
    onFocus: (event: FocusEvent) => { own.onFocus?.(event); setOpen(true); },
    onBlur: (event: FocusEvent) => { own.onBlur?.(event); setOpen(false); },
    onPointerEnter: (event: PointerEvent) => { own.onPointerEnter?.(event); setOpen(true); },
    onPointerLeave: (event: PointerEvent) => { own.onPointerLeave?.(event); setOpen(false); },
    onKeyDown: (event: KeyboardEvent) => { own.onKeyDown?.(event); if (event.key === "Escape") setOpen(false); },
  });
  return (
    <span className="ui-tooltip-anchor">
      {trigger}
      <span className="ui-tooltip" role="tooltip" id={id} data-open={open ? "true" : undefined}>{content}</span>
    </span>
  );
}
