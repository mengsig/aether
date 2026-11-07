import { useEffect, useId, useRef, type ReactNode } from "react";
import { Button } from "./button.tsx";
import { classes, invariant } from "./classes.ts";

export interface DialogProps {
  readonly open: boolean;
  /** Called on Escape, the close button or a backdrop click; the owner sets `open` to false. */
  readonly onClose: () => void;
  readonly title: string;
  readonly description?: string;
  readonly children?: ReactNode;
  readonly footer?: ReactNode;
  readonly className?: string;
}

interface SurfaceProps extends DialogProps {
  readonly kind: "dialog" | "sheet";
}

/** Moves focus back to the element that opened the surface, if it is still in the page. */
function restoreFocus(origin: { current: HTMLElement | null }): void {
  const target = origin.current;
  origin.current = null;
  if (target !== null && target.isConnected) target.focus();
}

/** The native modal <dialog>: the browser traps focus; focus returns to the opener on close or unmount; `data-autofocus` picks the first focus. */
function ModalSurface({ kind, open, onClose, title, description, children, footer, className }: SurfaceProps) {
  invariant(title.trim().length > 0, `a ${kind} needs a title`);
  const ref = useRef<HTMLDialogElement>(null);
  const id = useId();
  const opener = useRef<HTMLElement | null>(null);
  useEffect(() => {
    const dialog = ref.current;
    invariant(dialog !== null, `the ${kind} element is mounted`);
    if (open && !dialog.open) {
      opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      dialog.showModal();
      dialog.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    }
    if (!open && dialog.open) {
      dialog.close();
      restoreFocus(opener);
    }
  }, [open, kind]);
  useEffect(() => () => restoreFocus(opener), []);
  return (
    <dialog ref={ref} className={classes("ui-dialog", className)} data-kind={kind} aria-labelledby={`${id}-title`}
      aria-describedby={description === undefined ? undefined : `${id}-description`}
      onCancel={(event) => { event.preventDefault(); onClose(); }}
      onClick={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      {open ? (
        <div className="ui-dialog-frame">
          <div className="ui-dialog-header">
            <h2 className="ui-dialog-title" id={`${id}-title`}>{title}</h2>
            <Button variant="quiet" size="sm" className="ui-icon-button" aria-label={`Close ${title}`} onClick={onClose}><CloseGlyph /></Button>
          </div>
          {description === undefined ? null : <p className="ui-dialog-description" id={`${id}-description`}>{description}</p>}
          <div className="ui-dialog-body">{children}</div>
          {footer === undefined ? null : <div className="ui-dialog-footer">{footer}</div>}
        </div>
      ) : null}
    </dialog>
  );
}

export function Dialog(props: DialogProps) {
  return <ModalSurface {...props} kind="dialog" />;
}

/** A modal panel that slides in from the inline end (the bottom on narrow screens). */
export function Sheet(props: DialogProps) {
  return <ModalSurface {...props} kind="sheet" />;
}

/** The 16 px cross of every close and dismiss button. */
export function CloseGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true" focusable="false">
      <path d="M4 4l8 8M12 4l-8 8" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
