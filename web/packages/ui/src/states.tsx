import type { CSSProperties, ReactNode } from "react";
import { Button } from "./button.tsx";
import { classes, invariant } from "./classes.ts";

/** A placeholder block while content loads; hidden from assistive tech, which reads aria-busy instead. */
export function Skeleton(props: { readonly width?: string; readonly height?: string; readonly className?: string }) {
  const style: CSSProperties = { inlineSize: props.width ?? "100%", blockSize: props.height ?? "1em" };
  return <span className={classes("ui-skeleton", props.className)} style={style} aria-hidden="true" />;
}

export interface EmptyStateProps {
  readonly title: string;
  readonly children?: ReactNode;
  readonly action?: ReactNode;
  readonly className?: string;
}

export function EmptyState({ title, children, action, className }: EmptyStateProps) {
  invariant(title.trim().length > 0, "an empty state needs a title");
  return (
    <section className={classes("ui-state", className)} data-kind="empty" aria-label={title}>
      <h2 className="ui-state-title">{title}</h2>
      {children === undefined ? null : <div className="ui-state-body">{children}</div>}
      {action === undefined ? null : <div className="ui-state-action">{action}</div>}
    </section>
  );
}

export interface ErrorStateProps {
  readonly title?: string;
  readonly message: string;
  readonly onRetry?: () => void;
  readonly retryLabel?: string;
  readonly className?: string;
}

/** A failure the user can see and, where it makes sense, retry; announced as an alert. */
export function ErrorState({ title = "Something went wrong", message, onRetry, retryLabel = "Try again", className }: ErrorStateProps) {
  invariant(message.trim().length > 0, "an error state needs the error's message");
  return (
    <section className={classes("ui-state", className)} data-kind="error" role="alert">
      <h2 className="ui-state-title">{title}</h2>
      <p className="ui-state-body">{message}</p>
      {onRetry === undefined ? null : <div className="ui-state-action"><Button onClick={onRetry}>{retryLabel}</Button></div>}
    </section>
  );
}
