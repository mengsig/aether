// Markdown's inline layer: code spans, emphasis, links, autolinks and $math$. Like the block layer it
// builds React elements from a fixed set of tags and keeps every piece of input in a text node.

import { Fragment, type ReactNode } from "react";
import { invariant } from "../fleet/invariant.ts";
import { prettyTex } from "./math.ts";

const MAX_INLINE_DEPTH = 5;
const MAX_URL_LENGTH = 2048;
const SAFE_SCHEMES = new Set(["http", "https", "mailto"]);
const LINK_PROPS = { target: "_blank", rel: "noopener noreferrer nofollow" } as const;
const NAMED_ENTITIES: Readonly<Record<string, string>> = { tab: "\t", newline: "\n", colon: ":", amp: "&", sol: "/" };
const ENTITY = /&(?:#x([0-9a-f]{1,6})|#(\d{1,7})|([a-z]{2,8}));?/gi;
const URL_SCHEME = /^([a-z][a-z0-9+.-]*):/i;

// ---------------------------------------------------------------------------------- URLs

function isInvisible(code: number): boolean {
  return code <= 0x20 || (code >= 0x7f && code <= 0xa0) || code === 0x1680 || (code >= 0x2000 && code <= 0x200f)
    || (code >= 0x2028 && code <= 0x202f) || (code >= 0x205f && code <= 0x2064) || code === 0x3000 || code === 0xfeff;
}

function decodeEntities(text: string): string {
  return text.replace(ENTITY, (whole, hex: string | undefined, dec: string | undefined, name: string | undefined) => {
    if (name !== undefined) return NAMED_ENTITIES[name.toLowerCase()] ?? whole;
    const code = hex !== undefined ? Number.parseInt(hex, 16) : Number.parseInt(dec ?? "", 10);
    return code <= 0x10ffff ? String.fromCodePoint(code) : whole;
  });
}

function stripInvisible(text: string): string {
  let clean = "";
  for (let i = 0; i < text.length; i++) if (!isInvisible(text.charCodeAt(i))) clean += text[i];
  return clean;
}

/** The URL, normalised, if it is http(s), mailto or a same-site path; otherwise null. */
export function safeHref(url: string): string | null {
  invariant(typeof url === "string", "safeHref takes a string");
  if (url.length > MAX_URL_LENGTH) return null;
  const clean = stripInvisible(decodeEntities(url));
  invariant(clean.length <= url.length, "normalising a URL never lengthens it");
  if (clean.startsWith("#")) return clean;
  if (clean.startsWith("/")) return clean[1] === "/" || clean[1] === "\\" ? null : clean;
  const scheme = URL_SCHEME.exec(clean)?.[1];
  return scheme !== undefined && SAFE_SCHEMES.has(scheme.toLowerCase()) ? clean : null;
}

// ---------------------------------------------------------------------------------- inline

interface Hit { readonly end: number; readonly node: ReactNode }

/** Per-call scan state; the memos keep a failed search from being repeated for every opener. */
interface Scan {
  readonly src: string;
  readonly depth: number;
  readonly links: boolean;
  readonly noCloser: Map<string, number>;
  readonly noRun: Set<number>;
  pairs: Int32Array | null;
  /** The React key for the next element: its index among its siblings. */
  key: number;
}

const isSpace = (code: number) => code === 32 || code === 10 || code === 9 || code === 13;
const WORD = /[\p{L}\p{N}_]/u;
function isWord(ch: string): boolean {
  const code = ch.charCodeAt(0);
  if (code < 128) return (code >= 48 && code <= 57) || (code >= 65 && code <= 90) || (code >= 97 && code <= 122) || code === 95;
  return WORD.test(ch);
}
const isPunctuation = (code: number) => (code >= 33 && code <= 47) || (code >= 58 && code <= 64) || (code >= 91 && code <= 96) || (code >= 123 && code <= 126);

function children(s: Scan, text: string, links: boolean): ReactNode {
  return s.depth >= MAX_INLINE_DEPTH ? text : parseInline(text, s.depth + 1, links);
}

export function backtickRun(src: string, from: number): number {
  let end = from;
  while (src.charCodeAt(end) === 96) end++;
  return end - from;
}

function findRun(s: Scan, size: number, from: number): number {
  if (s.noRun.has(size)) return -1;
  let p = s.src.indexOf("`", from);
  while (p >= 0) {
    const run = backtickRun(s.src, p);
    if (run === size) return p;
    p = s.src.indexOf("`", p + run);
  }
  s.noRun.add(size);
  return -1;
}

function tryCode(s: Scan, i: number): Hit {
  const size = backtickRun(s.src, i);
  const close = findRun(s, size, i + size);
  invariant(size > 0, "a code span starts on a backtick");
  if (close < 0) return { end: i + size, node: "`".repeat(size) };
  let code = s.src.slice(i + size, close);
  if (code.length > 2 && code.startsWith(" ") && code.endsWith(" ")) code = code.slice(1, -1);
  return { end: close + size, node: <code key={s.key} className="md-code">{code}</code> };
}

function validCloser(src: string, p: number, delim: string): boolean {
  if (isSpace(src.charCodeAt(p - 1))) return false;
  if (delim.length > 1) return true;
  const before = src[p - 1];
  const after = src[p + 1];
  if (before === delim || after === delim) return false;
  return !(delim === "_" && after !== undefined && isWord(after));
}

function findCloser(s: Scan, delim: string, from: number): number {
  invariant(from >= 0 && from <= s.src.length, `math scan from ${from} stays in the source`);
  const failedFrom = s.noCloser.get(delim);
  if (failedFrom !== undefined && from >= failedFrom) return -1;
  let p = s.src.indexOf(delim, from);
  while (p >= 0) {
    if (validCloser(s.src, p, delim)) return p;
    p = s.src.indexOf(delim, p + 1);
  }
  s.noCloser.set(delim, from);
  return -1;
}

function delimiterAt(src: string, i: number): string | null {
  const ch = src[i];
  if (ch === "~") return src.startsWith("~~", i) ? "~~" : null;
  if (ch === "_") return "_";
  if (src.startsWith("***", i)) return "***";
  return src.startsWith("**", i) ? "**" : "*";
}

function emphasis(delim: string, body: ReactNode, key: number): ReactNode {
  if (delim === "***") return <strong key={key}><em>{body}</em></strong>;
  if (delim === "**") return <strong key={key}>{body}</strong>;
  return delim === "~~" ? <del key={key}>{body}</del> : <em key={key}>{body}</em>;
}

function tryEmphasis(s: Scan, i: number): Hit | null {
  const delim = delimiterAt(s.src, i);
  if (delim === null) return null;
  const open = i + delim.length;
  const next = s.src[open];
  if (next === undefined || next === delim[0] || isSpace(next.charCodeAt(0))) return null;
  if (delim === "_" && i > 0 && isWord(s.src[i - 1] ?? "")) return null;
  const close = findCloser(s, delim, open + 1);
  if (close < 0) return null;
  invariant(close > open, "an emphasis span has content");
  return { end: close + delim.length, node: emphasis(delim, children(s, s.src.slice(open, close), s.links), s.key) };
}

function matchBrackets(src: string): Int32Array {
  const pairs = new Int32Array(src.length).fill(-1);
  const opens: number[] = [];
  for (let i = 0; i < src.length; i++) {
    const code = src.charCodeAt(i);
    if (code === 92) i++;
    else if (code === 91) opens.push(i);
    else if (code === 93) {
      const open = opens.pop();
      if (open !== undefined) pairs[open] = i;
    }
  }
  return pairs;
}

/** The end of a `(url)` destination starting at `from`, or -1; balanced parentheses, no spaces. */
function destinationEnd(src: string, from: number): number {
  const limit = Math.min(src.length, from + MAX_URL_LENGTH + 1);
  let nesting = 0;
  for (let p = from; p < limit; p++) {
    const code = src.charCodeAt(p);
    if (isSpace(code)) return -1;
    if (code === 40) nesting++;
    else if (code === 41 && nesting-- === 0) return p;
  }
  return -1;
}

function tryLink(s: Scan, i: number): Hit | null {
  s.pairs ??= matchBrackets(s.src);
  const close = s.pairs[i] ?? -1;
  if (close < 0 || s.src[close + 1] !== "(") return null;
  const urlEnd = destinationEnd(s.src, close + 2);
  if (urlEnd < 0 || close === i + 1) return null;
  invariant(urlEnd > close + 1, "a link destination ends after its opening parenthesis");
  const href = safeHref(s.src.slice(close + 2, urlEnd));
  const label = children(s, s.src.slice(i + 1, close), false);
  const node = href === null ? <Fragment key={s.key}>{label}</Fragment> : <a key={s.key} className="md-link" href={href} {...LINK_PROPS}>{label}</a>;
  return { end: urlEnd + 1, node };
}

function trimAutolink(url: string): string {
  let end = url.length;
  let parens = 0;
  for (let i = 0; i < end; i++) parens += url[i] === "(" ? 1 : url[i] === ")" ? -1 : 0;
  while (end > 0) {
    const ch = url[end - 1] ?? "";
    if (ch === ")" && parens < 0) parens++;
    else if (!isPunctuation(ch.charCodeAt(0)) || ch === "/" || ch === "(" || ch === ")") break;
    end--;
  }
  return url.slice(0, end);
}

function tryAutolink(s: Scan, i: number): Hit | null {
  if (!s.src.startsWith("http://", i) && !s.src.startsWith("https://", i)) return null;
  if (i > 0 && isWord(s.src[i - 1] ?? "")) return null;
  let end = i;
  while (end < s.src.length && !isSpace(s.src.charCodeAt(end)) && s.src[end] !== "<") end++;
  const url = trimAutolink(s.src.slice(i, end));
  const href = url.length > MAX_URL_LENGTH ? null : safeHref(url);
  invariant(url.length <= end - i, "a trimmed autolink is part of the scanned token");
  if (href === null) return { end: i + url.length, node: url };
  return { end: i + url.length, node: <a key={s.key} className="md-link" href={href} {...LINK_PROPS}>{url}</a> };
}

/** A `$` closes math when it follows a non-space and no digit follows it, so "$5 and $10" stays text. */
function findMathCloser(s: Scan, delim: "$" | "$$", from: number): number {
  const failedFrom = s.noCloser.get(delim);
  if (failedFrom !== undefined && from >= failedFrom) return -1;
  let p = s.src.indexOf(delim, from);
  while (p >= 0) {
    const after = s.src.charCodeAt(p + delim.length);
    if (!isSpace(s.src.charCodeAt(p - 1)) && s.src[p - 1] !== "\\" && !(after >= 48 && after <= 57)) return p;
    p = s.src.indexOf(delim, p + 1);
  }
  s.noCloser.set(delim, from);
  return -1;
}

function tryMath(s: Scan, i: number): Hit | null {
  const delim = s.src.startsWith("$$", i) ? "$$" : "$";
  const open = i + delim.length;
  const next = s.src.charCodeAt(open);
  if (Number.isNaN(next) || isSpace(next) || next === 36) return null;
  const close = findMathCloser(s, delim, open + 1);
  if (close < 0) return null;
  invariant(close > open, "a math span has content");
  const tex = s.src.slice(open, close);
  const className = delim === "$$" ? "md-math md-math-display" : "md-math";
  return { end: close + delim.length, node: <span key={s.key} className={className} role="math" aria-label={tex} title={tex}>{prettyTex(tex)}</span> };
}

function tryAt(s: Scan, i: number): Hit | null {
  switch (s.src.charCodeAt(i)) {
    case 10: return { end: i + 1, node: <br key={s.key} /> };
    case 96: return tryCode(s, i);
    case 42: case 95: case 126: return tryEmphasis(s, i);
    case 91: return s.links ? tryLink(s, i) : null;
    case 104: return s.links ? tryAutolink(s, i) : null;
    case 36: return tryMath(s, i);
    default: return null;
  }
}

export function parseInline(src: string, depth: number, links: boolean): ReactNode[] {
  invariant(depth <= MAX_INLINE_DEPTH + 1, "inline nesting is capped");
  const s: Scan = { src, depth, links, noCloser: new Map(), noRun: new Set(), pairs: null, key: 0 };
  const out: ReactNode[] = [];
  let textStart = 0;
  let i = 0;
  while (i < src.length) {
    if (src.charCodeAt(i) === 92 && isPunctuation(src.charCodeAt(i + 1))) {
      if (i > textStart) out.push(src.slice(textStart, i));
      textStart = i + 1;
      i += 2;
      continue;
    }
    s.key = out.length;
    const hit = tryAt(s, i);
    if (hit === null) { i++; continue; }
    invariant(hit.end > i, "an inline construct consumes input");
    if (i > textStart) out.push(src.slice(textStart, i));
    out.push(hit.node);
    i = textStart = hit.end;
  }
  if (textStart < src.length) out.push(src.slice(textStart));
  return out;
}
