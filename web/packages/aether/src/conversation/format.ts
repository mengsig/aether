// Numbers as the agent and conversation screens print them: cost, tokens, sizes and durations.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../fleet/invariant.ts";

/** Dollars; `+` when invocations were left out of the total, `≈` when every figure is a list-price estimate. */
export function formatCost(usage: aether.Usage): string {
  invariant(Number.isFinite(usage.cost_usd) && usage.cost_usd >= 0, `cost ${usage.cost_usd} is a real amount`);
  const digits = usage.cost_usd >= 1 ? 2 : usage.cost_usd >= 0.01 ? 3 : 4;
  const amount = `$${usage.cost_usd.toFixed(digits)}`;
  const estimated = usage.cost_basis === "list_price" ? "≈" : "";
  return `${estimated}${amount}${usage.cost_complete ? "" : "+"}`;
}

/** Why a cost may be incomplete, in words, or null when it is complete. */
export function costCaveat(usage: aether.Usage): string | null {
  if (usage.cost_complete) return usage.cost_basis === "list_price" ? "Estimated from list prices" : null;
  const gap = usage.cost_gap ?? "unknown";
  const why: Record<string, string> = {
    unreported: "an engine reported no cost", unpriced: "a model has no configured price",
    interrupted: "an invocation was interrupted", mixed: "several reasons", unknown: "an unknown reason",
  };
  return `Leaves out some work: ${why[gap] ?? gap}`;
}

export function formatTokens(count: number): string {
  invariant(Number.isFinite(count) && count >= 0, `token count ${count} is not negative`);
  if (count < 1_000) return String(Math.round(count));
  if (count < 1_000_000) return `${(count / 1_000).toFixed(count < 10_000 ? 1 : 0)}k`;
  return `${(count / 1_000_000).toFixed(1)}M`;
}

export function formatBytes(size: number): string {
  invariant(Number.isFinite(size) && size >= 0, `size ${size} is not negative`);
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MB`;
}

export function formatDuration(ms: number): string {
  invariant(Number.isFinite(ms) && ms >= 0, `duration ${ms} is not negative`);
  if (ms < 1_000) return `${Math.round(ms)} ms`;
  if (ms < 60_000) return `${(ms / 1_000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)} min ${Math.round((ms % 60_000) / 1_000)} s`;
}

/** A daemon timestamp as local date and time; the raw text when it does not parse. */
export function formatTime(at: string): string {
  const time = Date.parse(at);
  if (Number.isNaN(time)) return at;
  return new Date(time).toLocaleString("en", { dateStyle: "medium", timeStyle: "short" });
}
