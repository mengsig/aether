import { cloneElement, useEffect, useId, useState, type FocusEvent, type ReactElement } from "react";
import { invariant } from "./classes.ts";

interface TriggerProps {
  "aria-describedby"?: string;
  onFocus?: (event: FocusEvent) => void;
  onBlur?: (event: FocusEvent) => void;
}

/**
 * A description shown on hover and focus (WCAG 1.4.13): Escape dismisses it wherever focus is,
 * and the pointer can move onto it without closing it. The trigger stays focusable.
 */
export function Tooltip({ content, children }: { readonly content: string; readonly children: ReactElement<TriggerProps> }) {
  invariant(content.trim().length > 0, "a tooltip needs text");
  const id = useId();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const open = (hovered || focused) && !dismissed;
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: globalThis.KeyboardEvent) => { if (event.key === "Escape") setDismissed(true); };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [open]);
  const own = children.props;
  const trigger = cloneElement(children, {
    "aria-describedby": [own["aria-describedby"], id].filter(Boolean).join(" "),
    onFocus: (event: FocusEvent) => { own.onFocus?.(event); setFocused(true); setDismissed(false); },
    onBlur: (event: FocusEvent) => { own.onBlur?.(event); setFocused(false); },
  });
  // Hover is tracked on the anchor, which holds the tooltip too, so moving onto the text keeps it open.
  return (
    <span className="ui-tooltip-anchor" onPointerEnter={() => { setHovered(true); setDismissed(false); }}
      onPointerLeave={() => setHovered(false)}>
      {trigger}
      <span className="ui-tooltip" role="tooltip" id={id} data-open={open ? "true" : undefined}>{content}</span>
    </span>
  );
}
