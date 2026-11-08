import { Button } from "./button.tsx";
import { invariant } from "./classes.ts";
import { CloseGlyph } from "./dialog.tsx";
import type { Tone } from "./badge.tsx";

export interface ToastMessage {
  readonly id: string | number;
  readonly title: string;
  readonly body?: string;
  readonly tone?: Exclude<Tone, "neutral" | "accent">;
}

export function Toast({ toast, onDismiss }: { readonly toast: ToastMessage; readonly onDismiss: (id: ToastMessage["id"]) => void }) {
  invariant(toast.title.trim().length > 0, "a toast needs a title");
  const urgent = toast.tone === "danger";
  return (
    <div className="ui-toast" data-tone={toast.tone ?? "info"} role={urgent ? "alert" : "status"}>
      <div className="ui-toast-text">
        <p className="ui-toast-title">{toast.title}</p>
        {toast.body === undefined ? null : <p className="ui-toast-body">{toast.body}</p>}
      </div>
      <Button variant="quiet" size="sm" className="ui-icon-button" aria-label={`Dismiss: ${toast.title}`} onClick={() => onDismiss(toast.id)}><CloseGlyph /></Button>
    </div>
  );
}

export interface ToastRegionProps {
  readonly toasts: readonly ToastMessage[];
  readonly onDismiss: (id: ToastMessage["id"]) => void;
  /** Offered as "Dismiss all" once more than one toast shows. */
  readonly onDismissAll?: (() => void) | undefined;
}

/** The always-mounted live region toasts appear in, so screen readers hear each one. */
export function ToastRegion({ toasts, onDismiss, onDismissAll }: ToastRegionProps) {
  invariant(new Set(toasts.map((toast) => toast.id)).size === toasts.length, "toast ids are unique");
  return (
    <section className="ui-toasts" aria-label="Notifications" aria-live="polite">
      {toasts.length > 1 && onDismissAll !== undefined
        ? <Button variant="quiet" size="sm" className="ui-toasts-clear" onClick={onDismissAll}>Dismiss all</Button>
        : null}
      {toasts.map((toast) => <Toast key={toast.id} toast={toast} onDismiss={onDismiss} />)}
    </section>
  );
}
