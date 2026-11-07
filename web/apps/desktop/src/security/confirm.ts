// The native confirmation of a privileged action (CONTRACTS.md §2.4). All wording lives here;
// a renderer value only fills a slot, escaped, and only a descriptive label is ever cut.

import { MAX_CONFIRMATION_LINES, MAX_LABEL_CHARS } from "./limits.ts";

/** The daemon a confirmation names, as main discovered it. */
export interface DaemonIdentity {
  readonly kind: "aether" | "archon";
  readonly label: string;
  readonly socket: string;
}

/** What the native dialog shows: a short question, the detail and the confirming button. */
export interface Confirmation {
  readonly title: string;
  readonly message: string;
  readonly detail: string;
  readonly confirm: string;
}

/** One changed settings key, as main read its old value and the patch names its new one. */
export interface SettingChange {
  /** A key from main's own tables. */
  readonly key: string;
  /** A renderer-named entry of a table setting, such as one pipeline's limit. */
  readonly entry?: string;
  readonly old: unknown;
  readonly new: unknown;
}

export class ConfirmationError extends Error {
  override readonly name = "ConfirmationError";
}

// Control, format (bidi overrides, zero-width) and separator characters, plus the quote and
// backslash the escaping itself uses.
const UNSAFE = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}"\\]/gu;

function escapeChar(char: string): string {
  if (char === '"' || char === "\\") return `\\${char}`;
  const code = char.codePointAt(0);
  if (code === undefined) throw new ConfirmationError("an unsafe character has a code point");
  return `\\u{${code.toString(16).padStart(4, "0")}}`;
}

/** `value` quoted, with every character that could alter the surrounding text escaped. */
export function valueSlot(value: string): string {
  if (typeof value !== "string") throw new ConfirmationError("a slot value is a string");
  const escaped = `"${value.replace(UNSAFE, escapeChar)}"`;
  if (/[\n\r]/.test(escaped)) throw new ConfirmationError("an escaped slot holds no line break");
  return escaped;
}

/** A descriptive label: cut to MAX_LABEL_CHARS characters with an ellipsis, then escaped. */
export function labelSlot(value: string): string {
  const chars = [...value];
  if (chars.length <= MAX_LABEL_CHARS) return valueSlot(value);
  return `${valueSlot(chars.slice(0, MAX_LABEL_CHARS).join(""))}…`;
}

/** A settings value: unset, a JSON scalar, or JSON for a list or table. Never cut. */
export function settingSlot(value: unknown): string {
  if (value === undefined || value === null) return "unset";
  if (typeof value === "string") return valueSlot(value);
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  const json = JSON.stringify(value);
  if (json === undefined) throw new ConfirmationError("a settings value is JSON");
  return json.replace(UNSAFE, (char) => (char === '"' || char === "\\" ? char : escapeChar(char)));
}

export function daemonSlot(daemon: DaemonIdentity): string {
  return `${valueSlot(daemon.label)} (${valueSlot(daemon.socket)})`;
}

// Settings keys come from main's own key tables; nothing else may name a line.
const KEY = /^[a-z][a-z0-9_]*(\.[a-z0-9_]+)*$/;

/** One line per changed key, `key: old → new`, the whole list or a refusal. */
export function changesBlock(changes: readonly SettingChange[]): string {
  if (changes.length === 0) throw new ConfirmationError("The patch changes nothing");
  const lines = changes.map((change) => {
    if (!KEY.test(change.key)) throw new ConfirmationError(`Not a settings key: ${valueSlot(change.key)}`);
    const key = change.entry === undefined ? change.key : `${change.key}[${valueSlot(change.entry)}]`;
    return `${key}: ${settingSlot(change.old)} → ${settingSlot(change.new)}`;
  });
  return lines.join("\n");
}

type Slots = Readonly<Record<string, string>>;

/**
 * Fills `{name}` slots in one pass, so text inside a value is never read as
 * a slot. Every slot the template names must be given, and only those.
 */
export function fill(template: string, slots: Slots): string {
  const used = new Set<string>();
  const text = template.replace(/\{([a-z]+)\}/g, (_match, name: string) => {
    const value = slots[name];
    if (value === undefined) throw new ConfirmationError(`the template slot {${name}} has no value`);
    used.add(name);
    return value;
  });
  const unused = Object.keys(slots).filter((name) => !used.has(name));
  if (unused.length > 0) throw new ConfirmationError(`slots the template does not name: ${unused.join(", ")}`);
  return text;
}

/** A confirmation from one of TEMPLATES, refused when it would not fit in full. */
export function confirmation(template: Template, slots: Slots): Confirmation {
  const detail = fill(template.detail, slots);
  const lines = detail.split("\n").length;
  if (lines > MAX_CONFIRMATION_LINES) {
    throw new ConfirmationError(`This change needs ${lines} lines to show in full; send it as smaller patches`);
  }
  return { title: template.title, message: template.message, detail, confirm: template.confirm };
}

export interface Template {
  readonly title: string;
  readonly message: string;
  readonly detail: string;
  readonly confirm: string;
}

const ON = "\nDaemon: {daemon}";

export const TEMPLATES = {
  "memory.accept": { title: "Accept memory", message: "Accept this memory?", confirm: "Accept",
    detail: `Memory {memory} becomes something agents recall.${ON}` },
  "memory.reject": { title: "Reject memory", message: "Reject this memory?", confirm: "Reject",
    detail: `Memory {memory} is set aside and never recalled.${ON}` },
  "device.pair": { title: "Pair device", message: "Start pairing a device?", confirm: "Start pairing",
    detail: `Pair device {name} as {role}.${ON}` },
  "device.revoke": { title: "Revoke device", message: "Revoke this device?", confirm: "Revoke",
    detail: `Device {device} can no longer connect.${ON}` },
  "remote.enable": { title: "Remote access", message: "Turn on remote access?", confirm: "Turn on",
    detail: `Paired devices can reach this daemon through its remote listener.${ON}` },
  "remote.disable": { title: "Remote access", message: "Turn off remote access?", confirm: "Turn off",
    detail: `The remote listener closes and paired devices disconnect.${ON}` },
  "remote.mount": { title: "Mount Archon", message: "Serve Archon through the remote listener?", confirm: "Mount",
    detail: `Prefix {prefix} forwards to port {port} on this machine.${ON}` },
  "noesis.configure": { title: "Noesis settings", message: "Change these Noesis settings?", confirm: "Change",
    detail: `{changes}${ON}` },
  "archon.merge": { title: "Merge item", message: "Merge this item?", confirm: "Merge",
    detail: `Item {item} merges into its base branch.${ON}` },
  "archon.unblock": { title: "Unblock item", message: "Retry this blocked item?", confirm: "Retry",
    detail: `Item {item} runs again from where it stopped.${ON}` },
  "archon.settings": { title: "Archon settings", message: "Change these Archon settings?", confirm: "Change",
    detail: `{changes}${ON}` },
  "external.open": { title: "Open link", message: "Open this link in your browser?", confirm: "Open in browser",
    detail: "The page is on {origin}, a site Aether does not know.\nLink: {url}" },
  "engine.login": { title: "Sign in", message: "Open a terminal to sign in?", confirm: "Open terminal",
    detail: `A terminal runs {command} to sign in to {engine}. Your sign-in stays with that tool.${ON}` },
} as const satisfies Readonly<Record<string, Template>>;

export type TemplateName = keyof typeof TEMPLATES;
