// A small Markdown renderer that is safe by construction: it builds React elements from a fixed
// set of tags and puts every piece of input in a text node, so input can never become markup.

import { Fragment, useMemo, type ReactNode } from "react";
import { invariant } from "./classes.ts";

const MAX_BLOCK_DEPTH = 4;
const MAX_INLINE_DEPTH = 5;
const MAX_URL_LENGTH = 2048;
const SAFE_SCHEMES = new Set(["http", "https", "mailto"]);
const HEADING_TAGS = ["h3", "h4", "h5", "h6"] as const;
const LINK_PROPS = { target: "_blank", rel: "noopener noreferrer nofollow" } as const;
const NAMED_ENTITIES: Readonly<Record<string, string>> = { tab: "\t", newline: "\n", colon: ":", amp: "&", sol: "/" };
const ENTITY = /&(?:#x([0-9a-f]{1,6})|#(\d{1,7})|([a-z]{2,8}));?/gi;
const LANG = /^[\w+#.-]{1,32}$/;
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

function backtickRun(src: string, from: number): number {
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

function tryAt(s: Scan, i: number): Hit | null {
  switch (s.src.charCodeAt(i)) {
    case 10: return { end: i + 1, node: <br key={s.key} /> };
    case 96: return tryCode(s, i);
    case 42: case 95: case 126: return tryEmphasis(s, i);
    case 91: return s.links ? tryLink(s, i) : null;
    case 104: return s.links ? tryAutolink(s, i) : null;
    default: return null;
  }
}

function parseInline(src: string, depth: number, links: boolean): ReactNode[] {
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

// ---------------------------------------------------------------------------------- blocks

interface Block { readonly node: ReactNode; readonly next: number }
interface Item { readonly indent: number; readonly ordered: boolean; readonly number: number; readonly text: string }
interface ListItem { text: string; sub: ListNode | null }
interface ListNode { readonly ordered: boolean; readonly start: number; readonly items: ListItem[] }

const isBlank = (line: string) => line.trim() === "";

function fenceOf(line: string): { ticks: number; lang: string | undefined } | null {
  const text = line.trimStart();
  const ticks = backtickRun(text, 0);
  const info = text.slice(ticks).trim();
  if (ticks < 3 || line.length - text.length > 3 || info.includes("`")) return null;
  const lang = info.split(/\s/, 1)[0] ?? "";
  return { ticks, lang: LANG.test(lang) ? lang : undefined };
}

function headingOf(line: string): { level: number; text: string } | null {
  const text = line.trimStart();
  let level = 0;
  while (text[level] === "#") level++;
  if (level < 1 || level > 6 || line.length - text.length > 3) return null;
  if (level < text.length && text[level] !== " " && text[level] !== "\t") return null;
  let body = text.slice(level).trim();
  let end = body.length;
  while (end > 0 && body[end - 1] === "#") end--;
  if (end < body.length && (end === 0 || body[end - 1] === " ")) body = body.slice(0, end).trimEnd();
  return { level, text: body };
}

function isRule(line: string): boolean {
  const text = line.trim();
  const mark = text[0];
  if (text.length < 3 || (mark !== "-" && mark !== "*" && mark !== "_")) return false;
  return text.split(mark).every((part) => part === "");
}

function quoteBody(line: string): string | null {
  const text = line.trimStart();
  if (text[0] !== ">" || line.length - text.length > 3) return null;
  return text.startsWith("> ") ? text.slice(2) : text.slice(1);
}

function itemOf(line: string): Item | null {
  let indent = 0;
  let p = 0;
  for (; p < line.length && (line[p] === " " || line[p] === "\t"); p++) indent += line[p] === "\t" ? 4 : 1;
  let q = p;
  while (q - p < 9 && line.charCodeAt(q) >= 48 && line.charCodeAt(q) <= 57) q++;
  const marker = line[q];
  const bullet = q === p && (marker === "-" || marker === "*" || marker === "+");
  const numbered = q > p && (marker === "." || marker === ")");
  const gap = line[q + 1];
  if (!(bullet || numbered) || (gap !== " " && gap !== "\t")) return null;
  return { indent, ordered: numbered, number: numbered ? Number.parseInt(line.slice(p, q), 10) : 1, text: line.slice(q + 2) };
}

function startsBlock(line: string, depth: number): boolean {
  if (fenceOf(line) !== null || headingOf(line) !== null || isRule(line)) return true;
  if (depth < MAX_BLOCK_DEPTH && quoteBody(line) !== null) return true;
  const item = itemOf(line);
  return item !== null && (!item.ordered || item.number === 1);
}

function readFence(lines: string[], i: number, ticks: number, lang: string | undefined): Block {
  let end = i + 1;
  while (end < lines.length) {
    const text = (lines[end] ?? "").trim();
    if (text.length >= ticks && backtickRun(text, 0) === text.length) break;
    end++;
  }
  invariant(end > i, "a fenced block spans its opening line");
  const code = lines.slice(i + 1, end).join("\n");
  return { node: <pre className="md-pre"><code data-lang={lang}>{code}</code></pre>, next: end + 1 };
}

function readQuote(lines: string[], i: number, depth: number): Block {
  const inner: string[] = [];
  for (let end = i; end < lines.length; end++) {
    const body = quoteBody(lines[end] ?? "");
    if (body === null) break;
    inner.push(body);
  }
  invariant(inner.length > 0, "a quote has at least its first line");
  return { node: <blockquote className="md-quote">{parseBlocks(inner, depth + 1)}</blockquote>, next: i + inner.length };
}

function newList(item: Item): ListNode {
  return { ordered: item.ordered, start: item.number, items: [{ text: item.text, sub: null }] };
}

function readList(lines: string[], i: number): Block {
  const first = itemOf(lines[i] ?? "");
  invariant(first !== null, "a list starts on an item line");
  const root = newList(first);
  let top = root.items[0] as ListItem;
  let last = top;
  let j = i + 1;
  for (; j < lines.length; j++) {
    const line = lines[j] ?? "";
    const item = itemOf(line);
    if (isBlank(line) && itemOf(lines[j + 1] ?? "") !== null) continue;
    if (item === null && (isBlank(line) || line.length - line.trimStart().length <= first.indent)) break;
    if (item === null) { last.text += `\n${line.trim()}`; continue; }
    if (item.indent >= first.indent + 2) {
      if (top.sub === null) top.sub = newList(item);
      else top.sub.items.push({ text: item.text, sub: null });
      last = top.sub.items.at(-1) as ListItem;
    } else if (item.ordered !== first.ordered) break;
    else root.items.push(top = last = { text: item.text, sub: null });
  }
  invariant(j > i, "a list consumes its first line");
  return { node: renderList(root), next: j };
}

function renderList(list: ListNode): ReactNode {
  const items = list.items.map((item, k) => (
    <li key={k}>{parseInline(item.text, 0, true)}{item.sub === null ? null : renderList(item.sub)}</li>
  ));
  return list.ordered ? <ol start={list.start === 1 ? undefined : list.start}>{items}</ol> : <ul>{items}</ul>;
}

function readParagraph(lines: string[], i: number, depth: number): Block {
  let end = i + 1;
  while (end < lines.length && !isBlank(lines[end] ?? "") && !startsBlock(lines[end] ?? "", depth)) end++;
  invariant(end > i, "a paragraph has at least one line");
  return { node: <p>{parseInline(lines.slice(i, end).join("\n"), 0, true)}</p>, next: end };
}

function readBlock(lines: string[], i: number, depth: number): Block {
  const line = lines[i] ?? "";
  const fence = fenceOf(line);
  if (fence !== null) return readFence(lines, i, fence.ticks, fence.lang);
  const heading = headingOf(line);
  if (heading !== null) {
    const Tag = HEADING_TAGS[Math.min(heading.level - 1, HEADING_TAGS.length - 1)] ?? "h6";
    return { node: <Tag>{parseInline(heading.text, 0, true)}</Tag>, next: i + 1 };
  }
  if (isRule(line)) return { node: <hr />, next: i + 1 };
  if (depth < MAX_BLOCK_DEPTH && quoteBody(line) !== null) return readQuote(lines, i, depth);
  return itemOf(line) !== null ? readList(lines, i) : readParagraph(lines, i, depth);
}

function parseBlocks(lines: string[], depth: number): ReactNode[] {
  invariant(depth <= MAX_BLOCK_DEPTH, "block nesting is capped");
  const out: ReactNode[] = [];
  for (let i = 0; i < lines.length;) {
    if (isBlank(lines[i] ?? "")) { i++; continue; }
    const block = readBlock(lines, i, depth);
    invariant(block.next > i, "every block consumes input");
    out.push(<Fragment key={out.length}>{block.node}</Fragment>);
    i = block.next;
  }
  return out;
}

export function Markdown(props: { readonly text: string }): ReactNode {
  const blocks = useMemo(() => parseBlocks(props.text.replace(/\r\n?/g, "\n").split("\n"), 0), [props.text]);
  return <div className="md">{blocks}</div>;
}
