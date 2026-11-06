// What a collapsed tool row says about its call: a glyph for the kind of tool and a one-line
// summary of its arguments, read from the input as the transcript or the live stream carries it.

import { invariant } from "../fleet/invariant.ts";

const SUMMARY_LENGTH = 140;
/** The argument a person recognises a call by, in order of preference. */
const KEY_ARGUMENTS = ["command", "cmd", "file_path", "path", "pattern", "query", "url", "description", "prompt", "content", "message"];
const GLYPHS: readonly (readonly [RegExp, string])[] = [
  [/bash|shell|exec|command|terminal|run/i, "$"],
  [/write|edit|patch|apply|create|replace/i, "✎"],
  [/grep|search|find|glob|query/i, "⌕"],
  [/read|view|cat|open|list|^ls$/i, "≡"],
  [/web|fetch|http|url|browse/i, "↗"],
  [/agent|task|delegate|spawn|message|send/i, "⇢"],
];

/** A glyph for the tool's kind; a plain dot for one it does not recognise. */
export function toolGlyph(name: string): string {
  for (const [pattern, glyph] of GLYPHS) if (pattern.test(name)) return glyph;
  return "•";
}

function oneLine(text: string): string {
  const line = text.replace(/\s+/g, " ").trim();
  return line.length > SUMMARY_LENGTH ? `${line.slice(0, SUMMARY_LENGTH - 1)}…` : line;
}

function fromObject(value: Readonly<Record<string, unknown>>): string {
  for (const key of KEY_ARGUMENTS) {
    const field = value[key];
    if (typeof field === "string" && field.trim() !== "") return oneLine(field);
  }
  const entries = Object.entries(value).filter(([, field]) => field !== null && typeof field !== "object");
  return oneLine(entries.map(([key, field]) => `${key}: ${String(field)}`).join(", "));
}

/** A live call's input is JSON still being written: take the first key argument it already holds. */
function fromPartial(input: string): string {
  for (const key of KEY_ARGUMENTS) {
    const match = new RegExp(`"${key}"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)`).exec(input);
    if (match?.[1]) return oneLine(match[1].replace(/\\n/g, " ").replace(/\\(.)/g, "$1"));
  }
  return "";
}

/** One line that says what the call does, or "" when its input says nothing readable. */
export function argumentSummary(input: string): string {
  invariant(typeof input === "string", "a tool's input arrives as text");
  const text = input.trim();
  if (text === "" || text === "{}") return "";
  if (!text.startsWith("{") && !text.startsWith("[")) return oneLine(text);
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    // Expected while a call streams: its JSON is not complete yet.
    if (!(error instanceof SyntaxError)) throw error;
    return fromPartial(text);
  }
  if (parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)) return fromObject(parsed as Record<string, unknown>);
  return oneLine(text);
}
