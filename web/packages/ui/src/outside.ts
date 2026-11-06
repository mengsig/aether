import { useEffect, useRef, type RefObject } from "react";
import { invariant } from "./classes.ts";

/** While `active`, a pointer press outside `root` calls `dismiss`: the close rule of every popup in the kit. */
export function useDismissOnOutside(root: RefObject<HTMLElement | null>, active: boolean, dismiss: () => void): void {
  invariant(typeof dismiss === "function", "an outside press needs a dismiss callback");
  const latest = useRef(dismiss);
  useEffect(() => {
    latest.current = dismiss;
  });
  useEffect(() => {
    if (!active) return undefined;
    const outside = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || !root.current?.contains(event.target)) latest.current();
    };
    document.addEventListener("pointerdown", outside);
    return () => document.removeEventListener("pointerdown", outside);
  }, [active, root]);
}
