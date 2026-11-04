import type { ReadEntry } from "@aether-ui/core";
import { ErrorState, Skeleton } from "@aether-ui/ui";
import { invariant } from "../fleet/invariant.ts";

export function records<T>(value: unknown): T[] {
  if (value === null || value === undefined) return [];
  invariant(Array.isArray(value), "the daemon returned a list");
  invariant(value.every(item => item !== null && typeof item === "object"), "list entries are records");
  return value as T[];
}

export function ReadNote({ entry, retry }: { entry: ReadEntry | undefined; retry: () => unknown }) {
  if (entry?.error) return <ErrorState message={entry.error} onRetry={() => { void retry(); }} />;
  if (!entry || entry.requested > entry.issued) return <div aria-busy="true"><Skeleton /></div>;
  return null;
}
