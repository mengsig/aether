// `#/` home: an overview of every connection, each tile a number, a line of context and a link into
// the screen that owns it. With nothing paired it is one guided start instead.

import { useNow, useSelector } from "@aether-ui/core";
import { EmptyState } from "@aether-ui/ui";
import { useMemo, type ReactNode } from "react";
import { ActivityTile, AgentsTile, NeedsTile, SpendTile } from "./aether-tiles.tsx";
import { GithubTile, LearnTile, QueueTile } from "./archon-tiles.tsx";
import { deriveHome, type HomeView } from "./derive.ts";
import { invariant } from "./invariant.ts";
import { startOfDay } from "./model.ts";
import { homeReads, noesisReadConn, sameHomeSources, selectHomeSources, useHomeReads, type HomeRead } from "./sources.ts";
import { screenMode } from "./steps.ts";
import { LoadingTile } from "./tile-parts.tsx";
import "./home.css";

/** How often relative times and the hourly buckets move on. */
export const TICK_MS = 30_000;
const NO_READS: readonly HomeRead[] = [];

const TILES: readonly { readonly area: string; readonly label: string; readonly Tile: (props: { readonly view: HomeView }) => ReactNode }[] = [
  { area: "needs", label: "Needs you", Tile: NeedsTile },
  { area: "agents", label: "Agents", Tile: AgentsTile },
  { area: "spend", label: "Spend today", Tile: SpendTile },
  { area: "activity", label: "Recent activity", Tile: ActivityTile },
  { area: "queue", label: "Archon queue", Tile: QueueTile },
  { area: "learn", label: "Learn", Tile: LearnTile },
  { area: "github", label: "GitHub", Tile: GithubTile },
];

function ConnectFirst() {
  return (
    <EmptyState title="Connect to Aether" className="home-start"
      action={<span className="home-start-links"><a className="ui-button" data-variant="primary" href="#/pair/aether">Pair with Aether</a><a className="ui-link" href="#/pair/archon">Pair with Archon</a></span>}>
      <p>Aether runs your agents; Archon queues work for them from your repositories.</p>
      <p>Pair this device with either daemon and this page fills with what is running, what waits on you and what it costs.</p>
    </EmptyState>
  );
}

export function HomeScreen() {
  const now = useNow(TICK_MS);
  const dayStart = startOfDay(now);
  const select = useMemo(() => selectHomeSources(dayStart), [dayStart]);
  const sources = useSelector(select, sameHomeSources);
  const mode = screenMode(sources.connections);
  const noesisConn = noesisReadConn(sources);
  const reads = useMemo(() => (mode === "tiles" ? homeReads(sources, dayStart, noesisConn) : NO_READS), [mode, sources, dayStart, noesisConn]);
  const failures = useHomeReads(reads);
  const view = useMemo(() => deriveHome(sources, now, dayStart, failures), [sources, now, dayStart, failures]);
  invariant(TILES.length === new Set(TILES.map((tile) => tile.area)).size, "each tile has its own place in the grid");
  return (
    <section className="home" data-testid="home-dashboard" data-mode={mode} aria-label="Overview" aria-busy={mode === "loading" ? true : undefined}>
      <h2 className="ui-visually-hidden">Overview</h2>
      {mode === "empty" ? <ConnectFirst /> : (
        <div className="home-grid">
          {TILES.map(({ area, label, Tile }) => (
            <div key={area} className="home-cell" data-area={area}>{mode === "loading" ? <LoadingTile label={label} /> : <Tile view={view} />}</div>
          ))}
        </div>
      )}
    </section>
  );
}
