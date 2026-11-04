// How a screen shows one of core's stored rereads: a skeleton while the first one is in flight, an
// error with a retry when it failed, the stored value otherwise. It keeps no copy of the value.

import type { ReadEntry } from "@aether-ui/core";
import { ErrorState, Skeleton } from "@aether-ui/ui";
import type { ReactNode } from "react";
import { invariant } from "../kit/invariant.ts";

export interface ReadViewProps {
  readonly label: string;
  readonly entry: ReadEntry | undefined;
  readonly onRetry: () => void;
  readonly children: (value: unknown) => ReactNode;
}

function Loading({ label }: { readonly label: string }) {
  return (
    <div className="st-loading" aria-busy="true" aria-label={`Loading ${label}`}>
      <Skeleton height="var(--space-10)" />
      <Skeleton height="var(--space-10)" width="70%" />
    </div>
  );
}

export function ReadView({ label, entry, onRetry, children }: ReadViewProps) {
  invariant(label.trim() !== "", "a read view names what it shows");
  if (entry === undefined || (entry.value === null && entry.error === null)) return <Loading label={label} />;
  if (entry.value === null) {
    invariant(entry.error !== null, "an empty entry has failed");
    return <ErrorState title={`Could not load ${label}`} message={entry.error} onRetry={onRetry} retryLabel="Retry" />;
  }
  return (
    <>
      {entry.error === null ? null : <p className="st-note" role="alert" data-state="failed">The last refresh failed: {entry.error}</p>}
      {children(entry.value)}
    </>
  );
}

/** The reply as a list, or null when the daemon sent something else. */
export function listOf<T>(value: unknown): readonly T[] | null {
  return Array.isArray(value) ? (value as T[]) : null;
}

/** The reply as a record, or null when the daemon sent something else. */
export function recordOf<T extends object>(value: unknown): T | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? (value as T) : null;
}

export function Unexpected({ label }: { readonly label: string }) {
  return <ErrorState title={`Could not read ${label}`} message="The daemon's reply was not in the shape this app expects." />;
}
