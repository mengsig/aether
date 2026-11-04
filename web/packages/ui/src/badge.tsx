import type { CSSProperties, ReactNode } from "react";
import { classes, invariant } from "./classes.ts";

export type Tone = "neutral" | "accent" | "info" | "success" | "warning" | "danger";

export function Badge(props: { readonly tone?: Tone; readonly children: ReactNode; readonly className?: string }) {
  return <span className={classes("ui-badge", props.className)} data-tone={props.tone ?? "neutral"}>{props.children}</span>;
}

/** Every state with its own token in CONTRACTS.md §4. */
export const STATUSES = [
  "queued", "dispatching", "running", "waiting", "operator", "blocked", "succeeded", "failed", "cancelled",
  "pr-open", "pr-draft", "pr-merged", "pr-closed", "issue-open", "issue-closed",
  "mastery-unseen", "mastery-exposed", "mastery-demonstrated", "mastery-fragile", "mastery-misconception",
] as const;
export type Status = (typeof STATUSES)[number];
const KNOWN: ReadonlySet<string> = new Set(STATUSES);
const LIVE: ReadonlySet<Status> = new Set(["dispatching", "running"]);

/** The custom property that colours a status: run states are `--status-*`, the rest are named as they are. */
export function statusToken(status: Status): string {
  invariant(KNOWN.has(status), `unknown status ${status}`);
  const prefixed = status.startsWith("pr-") || status.startsWith("issue-") || status.startsWith("mastery-");
  return prefixed ? `--${status}` : `--status-${status}`;
}

function defaultLabel(status: Status): string {
  const word = status.slice(status.indexOf("-") + 1).replace("-", " ");
  return word.charAt(0).toUpperCase() + word.slice(1);
}

/** A status as text plus its colour, never colour alone; live states pulse unless motion is reduced. */
export function StatusPill(props: { readonly status: Status; readonly label?: string; readonly className?: string }) {
  const token = statusToken(props.status);
  const style = { "--pill-color": `var(${token})` } as CSSProperties;
  return (
    <span className={classes("ui-pill", props.className)} data-status={props.status} data-live={LIVE.has(props.status) ? "true" : undefined} style={style}>
      <span className="ui-pill-dot" aria-hidden="true" />
      {props.label ?? defaultLabel(props.status)}
    </span>
  );
}
