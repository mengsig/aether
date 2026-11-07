// The composer's `/` commands: what each one is, how a draft line is read against them, and the
// builders for the desktop's own commands, Archon's host commands and the transcript view modes.

import { navigate, routeHref } from "@aether-ui/core";
import type { aether, ConnId } from "@aether-ui/protocol";
import { DraftError } from "../controls/commands.ts";
import { invariant } from "../fleet/invariant.ts";
import { ARCHON_ORCHESTRATOR, CALM } from "../conversation/modes.ts";

export interface SlashChoice { readonly value: string; readonly label: string; readonly about?: string }

export interface SlashCommand {
  readonly name: string;
  /** The argument hint, as the TUI writes it: `<required>` and `[optional]`. */
  readonly args: string;
  readonly about: string;
  readonly group: string;
  readonly choices?: readonly SlashChoice[];
  /** Runs in the app; `null` sends the line to the agent exactly as typed. */
  readonly run: ((args: string) => void) | null;
}

/** What the menu offers for a draft: nothing, command names, or the choices of one command's argument. */
export type SlashQuery =
  | { readonly kind: "none" }
  | { readonly kind: "name"; readonly prefix: string }
  | { readonly kind: "argument"; readonly command: SlashCommand; readonly prefix: string };

const NONE: SlashQuery = { kind: "none" };

export function slashQuery(draft: string, commands: readonly SlashCommand[]): SlashQuery {
  if (!draft.startsWith("/") || draft.includes("\n")) return NONE;
  const body = draft.slice(1);
  const space = body.indexOf(" ");
  if (space < 0) return { kind: "name", prefix: body.toLowerCase() };
  const command = commands.find((c) => c.name === body.slice(0, space));
  const rest = body.slice(space + 1);
  if (!command?.choices || rest.includes(" ")) return NONE;
  return { kind: "argument", command, prefix: rest.toLowerCase() };
}

export function matchingCommands(commands: readonly SlashCommand[], prefix: string): readonly SlashCommand[] {
  invariant(prefix === prefix.toLowerCase(), "command names are matched in lower case");
  return commands.filter((c) => c.name.startsWith(prefix));
}

export function matchingChoices(command: SlashCommand, prefix: string): readonly SlashChoice[] {
  invariant(command.choices !== undefined, "only a command with choices offers them");
  return command.choices.filter((c) => c.value.toLowerCase().startsWith(prefix));
}

/** The known command a line runs and its argument text, or null for a line sent verbatim. */
export function parseLine(line: string, commands: readonly SlashCommand[]): { readonly command: SlashCommand; readonly args: string } | null {
  const match = /^\/(\S+)(?:\s+([\s\S]*))?$/.exec(line.trim());
  if (match === null) return null;
  const command = commands.find((c) => c.name === match[1]);
  return command === undefined ? null : { command, args: (match[2] ?? "").trim() };
}

/**
 * Why `line` stays here: without the view-mode catalog (`unavailable` says why, null when it is read),
 * a `/name` no command knows may toggle a user's own view mode, so it never reaches the agent as text.
 */
export function heldUnknown(line: string, commands: readonly SlashCommand[], unavailable: string | null): string | null {
  if (unavailable === null) return null;
  invariant(unavailable !== "", "a missing catalog has a reason");
  const name = /^\/(\S+)/.exec(line.trim())?.[1];
  if (name === undefined || !COMMAND_NAME.test(name) || commands.some((c) => c.name === name)) return null;
  return `${unavailable}, so /${name} is not sent: it may be a view mode`;
}

/** A command whose argument must be given is completed by Enter, not run bare. */
export const needsArgument = (command: SlashCommand): boolean => command.args.startsWith("<");

/** The daemon's view mode names (aether-core `valid_name`) fit this too. */
const COMMAND_NAME = /^[a-z][a-z0-9_-]*$/;

/** First-wins by name, so a later list never shadows an earlier one. */
export function mergeCommands(...lists: readonly (readonly SlashCommand[])[]): readonly SlashCommand[] {
  const byName = new Map<string, SlashCommand>();
  for (const command of lists.flat()) if (!byName.has(command.name)) byName.set(command.name, command);
  const merged = [...byName.values()];
  invariant(merged.every((c) => COMMAND_NAME.test(c.name)), "command names are lower-case words");
  return merged;
}

const SETTINGS_PAGES: readonly SlashChoice[] = [
  { value: "connections", label: "Connections", about: "the daemons this app talks to" },
  { value: "devices", label: "Devices", about: "paired devices and their standing" },
  { value: "remote", label: "Remote", about: "reach this daemon from another machine" },
  { value: "providers", label: "Providers", about: "model providers, sign-in and limits" },
  { value: "notifications", label: "Notifications", about: "what the app tells you about" },
];

/** The TUI's client commands that have a desktop screen, each opening that screen's route. */
export function clientCommands(conn: ConnId, agent: aether.Agent): readonly SlashCommand[] {
  invariant(conn !== "" && agent.id !== "", "client commands act on a named connection and agent");
  const controls = routeHref("#/controls/:conn/:agent", { conn, agent: agent.id });
  const open = (href: string) => () => navigate(href);
  const settings = (args: string) => {
    const page = args.trim();
    if (page !== "" && !SETTINGS_PAGES.some((p) => p.value === page)) throw new DraftError(`No settings page ${page}`);
    navigate(page === "" ? "#/settings" : `#/settings/${page}`);
  };
  const group = "App";
  return [
    { name: "settings", args: "[page]", about: "open the settings; a page name opens that page", group, choices: SETTINGS_PAGES, run: settings },
    { name: "usage", args: "", about: "providers, sign-in, health and account limits", group, run: open("#/settings/providers") },
    { name: "notifications", args: "", about: "what agents need you to see: questions, approvals, failures", group, run: open("#/inbox") },
    { name: "memory", args: "", about: "browse durable memories; pin or forget one", group, run: open(routeHref("#/memory/:conn", { conn })) },
    { name: "report", args: "", about: "what this agent's run did, under Records in its controls", group, run: open(controls) },
    { name: "artifacts", args: "", about: "this agent's artifacts, under Records in its controls", group, run: open(controls) },
    { name: "schedules", args: "", about: "recurring timers, under Schedules in its controls", group, run: open(controls) },
    { name: "approvals", args: "", about: "approval rules, under Approvals in its controls", group, run: open(controls) },
    { name: "doctor", args: "", about: "what is and is not working: connections and their state", group, run: open("#/settings/connections") },
    { name: "activity", args: "", about: "every daemon event as it happens", group, run: open("#/activity") },
    { name: "help", args: "", about: "keys and every screen, in the command palette (Ctrl+K)", group, run: openPalette },
  ];
}

/** The shell opens its command palette, the app's list of keys and screens, on Ctrl+K. */
function openPalette(): void {
  document.dispatchEvent(new KeyboardEvent("keydown", { key: "k", ctrlKey: true, bubbles: true }));
}

const ARCHON_COMMANDS: readonly (readonly [name: string, args: string, about: string])[] = [
  ["answer", "<n|id> <text>", "answer question n (a number picks one of its options)"],
  ["questions", "", "list open questions"],
  ["approve", "[n] [note]", "approve a proposal or report gate"],
  ["reject", "[n] [reason]", "reject a proposal or report gate"],
  ["merge", "<id> [--ignore-checks]", "merge the item's pull request now"],
  ["archon", "<command>", "change operator settings (see guide authority)"],
  ["status", "", "daemon, Aether, queue and spend"],
  ["queue", "[state ...]", "the queue, e.g. /queue running failed"],
  ["item", "<id>", "one queue item (its id or the id's start)"],
  ["repos", "", "the registered repositories"],
  ["lessons", "", "proposed lessons; accept or retract one with `archon lessons accept|retract <id>`"],
  ["update", "", "update Archon and Aether to the latest release; running agents are waited for"],
];

/** Archon's host commands, offered only to its orchestrator, which reads them from the line as typed. */
export function hostCommands(agent: aether.Agent): readonly SlashCommand[] {
  invariant(agent.role.trim() !== "", "an agent has a role");
  if (agent.role !== ARCHON_ORCHESTRATOR) return [];
  return ARCHON_COMMANDS.map(([name, args, about]) => ({ name, args, about, group: "Archon", run: null }));
}

export interface ModeInfo { readonly name: string; readonly label: string; readonly description: string }

/** Names a view mode toggle may not take: the commands above and the TUI's own. */
const TAKEN = new Set(["mode", "new", "model", "stop", "cancel", "pause", "resume", "wake", "restart", "compact", "clear", "quit",
  ...ARCHON_COMMANDS.map(([name]) => name), "settings", "usage", "notifications", "memory", "report", "artifacts", "schedules", "approvals", "doctor", "activity", "help"]);

/** Toggles known before the daemon's catalog arrives: the built-in focus and Archon's Calm. */
const KNOWN_TOGGLES = ["focus", CALM];

/**
 * `/mode <name>` sets a view mode; `/<name>` toggles a non-standard one with the standard view. Without
 * a catalog (`unavailable` says why) both still run here and refuse, so a view command never reaches the agent.
 */
export function modeCommands(modes: readonly ModeInfo[], choose: (name: string) => void, toggle: (name: string) => void, unavailable: string): readonly SlashCommand[] {
  invariant(new Set(modes.map((m) => m.name)).size === modes.length, "view mode names are unique");
  invariant(unavailable !== "", "a missing catalog has a reason");
  const group = "View";
  if (modes.length === 0) {
    const refuse = () => { throw new DraftError(unavailable); };
    return [
      { name: "mode", args: "<name>", about: "choose a transcript view mode", group, run: refuse },
      ...KNOWN_TOGGLES.map((name) => ({ name, args: "", about: unavailable, group, run: refuse })),
    ];
  }
  const pick = (args: string) => {
    const name = args.trim();
    if (!modes.some((m) => m.name === name)) throw new DraftError(`Choose a view mode: ${modes.map((m) => m.name).join(", ")}`);
    choose(name);
  };
  const choices = modes.map((m) => ({ value: m.name, label: m.label, about: m.description }));
  const toggles = modes.filter((m) => m.name !== "standard" && !TAKEN.has(m.name) && COMMAND_NAME.test(m.name));
  return [
    { name: "mode", args: "<name>", about: "choose a transcript view mode", group, choices, run: pick },
    ...toggles.map((m) => ({ name: m.name, args: "", about: `${m.label}: ${m.description} (again for standard)`, group, run: () => toggle(m.name) })),
  ];
}
