// Which state the overview is in, and the one next step a tile offers when it has nothing to show:
// pair a daemon, reconnect it, or start the thing the tile counts. Pure; nothing here reads a daemon.

import type { ConnectionInfo } from "@aether-ui/protocol";
import { invariant } from "./invariant.ts";

/** Whether any connection of a daemon is ready, is still connecting, is down, or none is paired. */
export type Presence = "none" | "connecting" | "offline" | "ready";
export type NoesisState = "none" | "connecting" | "offline" | "unsupported" | "loading" | "off" | "on";
export type TileKey = "agents" | "needs" | "queue" | "spend" | "activity" | "learn" | "github";
export interface Step { readonly label: string; readonly href: string }

const CONNECTING: ReadonlySet<string> = new Set(["connecting", "pairing"]);

export function presence(connections: readonly ConnectionInfo[], daemon: ConnectionInfo["daemon"]): Presence {
  const own = connections.filter((info) => info.daemon === daemon);
  if (own.length === 0) return "none";
  if (own.some((info) => info.state === "ready")) return "ready";
  return own.some((info) => CONNECTING.has(info.state)) ? "connecting" : "offline";
}

/** No connection at all is the guided start; nothing ready yet but something connecting is a loading page. */
export function screenMode(connections: readonly ConnectionInfo[]): "empty" | "loading" | "tiles" {
  if (connections.length === 0) return "empty";
  const states = connections.map((info) => info.state);
  invariant(states.every((state) => typeof state === "string"), "every connection has a state");
  if (states.includes("ready")) return "tiles";
  return states.some((state) => CONNECTING.has(state)) ? "loading" : "tiles";
}

/** Noesis on the connection it is read from; `status` is the `noesis_status` core read at ready. */
export function noesisState(info: ConnectionInfo | undefined, status: { readonly enabled?: unknown } | null): NoesisState {
  if (info === undefined) return "none";
  if (info.state !== "ready") return CONNECTING.has(info.state) ? "connecting" : "offline";
  if (!info.features.includes("noesis")) return "unsupported";
  if (status === null) return "loading";
  invariant(typeof status.enabled === "boolean", "noesis_status says whether Noesis is enabled");
  return status.enabled ? "on" : "off";
}

export interface Facts {
  readonly aether: Presence;
  readonly archon: Presence;
  readonly agents: number;
  readonly events: number;
  readonly items: number;
  readonly noesis: NoesisState;
  readonly sessions: number;
  readonly watched: number;
}

const PAIR_AETHER: Step = { label: "Connect to Aether", href: "#/pair/aether" };
const PAIR_ARCHON: Step = { label: "Connect to Archon", href: "#/pair/archon" };
const RECONNECT: Step = { label: "Check connections", href: "#/settings/connections" };
const START_AGENT: Step = { label: "Start an agent", href: "#/activity" };
const REGISTER_REPO: Step = { label: "Register a repository", href: "#/archon/queue" };
const WATCH_REPO: Step = { label: "Watch a repository", href: "#/github" };

/** The step a daemon's tiles offer until it is ready; null while it is still connecting. */
function connectStep(daemon: Presence, pair: Step): Step | null {
  if (daemon === "none") return pair;
  return daemon === "offline" ? RECONNECT : null;
}

/** The tile's quiet next step, or null when the tile has something real to show. */
export function nextStep(tile: TileKey, facts: Facts): Step | null {
  invariant([facts.agents, facts.events, facts.items, facts.sessions, facts.watched].every((n) => Number.isInteger(n) && n >= 0), "facts are counts");
  switch (tile) {
    case "agents":
    case "spend": return facts.aether !== "ready" ? connectStep(facts.aether, PAIR_AETHER) : facts.agents === 0 ? START_AGENT : null;
    case "activity": return facts.aether !== "ready" ? connectStep(facts.aether, PAIR_AETHER) : facts.events === 0 ? START_AGENT : null;
    case "needs": return facts.aether === "ready" || facts.archon === "ready" ? null : connectStep(facts.aether, PAIR_AETHER);
    case "queue": return facts.archon !== "ready" ? connectStep(facts.archon, PAIR_ARCHON) : facts.items === 0 ? REGISTER_REPO : null;
    case "github": return facts.archon !== "ready" ? connectStep(facts.archon, PAIR_ARCHON) : facts.watched === 0 ? WATCH_REPO : null;
    case "learn": return learnStep(facts);
  }
}

function learnStep(facts: Facts): Step | null {
  switch (facts.noesis) {
    case "none": return PAIR_AETHER;
    case "offline": return RECONNECT;
    case "unsupported": return { label: "Update Aether to use Noesis", href: "#/learn" };
    case "off": return { label: "Turn on Noesis", href: "#/learn" };
    case "on": return facts.sessions === 0 ? { label: "Start a session", href: "#/learn" } : null;
    case "connecting":
    case "loading": return null;
  }
}
