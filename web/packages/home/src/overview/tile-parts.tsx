// What every overview tile shares: the loading tile, the quiet next-step link and the words a tile
// shows in place of a number while its daemon is not connected.

import { Skeleton } from "@aether-ui/ui";
import type { Presence, Step } from "./steps.ts";
import { invariant } from "./invariant.ts";

export function LoadingTile(props: { readonly label: string }) {
  invariant(props.label !== "", "a loading tile names what it loads");
  return (
    <article className="ui-chart-tile home-tile home-loading" aria-busy="true" aria-label={`${props.label}, loading`}>
      <h3 className="ui-chart-tile-label">{props.label}</h3>
      <Skeleton width="40%" height="var(--text-2xl)" className="home-skeleton-value" />
      <Skeleton width="75%" />
      <Skeleton height="var(--space-8)" />
    </article>
  );
}

export function NextStep(props: { readonly step: Step }) {
  invariant(props.step.href.startsWith("#/"), `a next step links to a route, not ${props.step.href}`);
  return <a className="ui-link home-step" href={props.step.href}>{props.step.label}<span aria-hidden="true"> →</span></a>;
}

/** The tile's figure while its daemon is not ready: never a dash, always what is going on. */
export function presenceValue(daemon: Presence): string {
  invariant(daemon !== "ready", "a ready daemon shows its own figure");
  return daemon === "none" ? "Not connected" : daemon === "offline" ? "Offline" : "Connecting";
}

export const plural = (count: number, one: string, many = `${one}s`): string => `${count} ${count === 1 ? one : many}`;

/** The nonzero parts of a hint, joined, or the fallback when every part is zero. */
export function hintOf(parts: readonly (readonly [number, string])[], fallback: string): string {
  const shown = parts.filter(([count]) => count > 0).map(([, text]) => text);
  return shown.length === 0 ? fallback : shown.join(" · ");
}
