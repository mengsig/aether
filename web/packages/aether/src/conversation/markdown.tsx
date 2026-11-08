// A small Markdown renderer that is safe by construction: it builds React elements from a fixed
// set of tags and puts every piece of input in a text node, so input can never become markup.

import { MermaidDiagram } from "@aether-ui/ui";
import { Fragment, useMemo, type ReactNode } from "react";
import { invariant } from "../fleet/invariant.ts";
import { CopyButton } from "./CopyButton.tsx";
import { backtickRun, parseInline } from "./markdown-inline.tsx";
import { prettyTex } from "./math.ts";

export { safeHref } from "./markdown-inline.tsx";

const MAX_BLOCK_DEPTH = 4;
const HEADING_TAGS = ["h3", "h4", "h5", "h6"] as const;
const LANG = /^[\w+#.-]{1,32}$/;

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
  if (fenceOf(line) !== null || headingOf(line) !== null || isRule(line) || isMathFence(line)) return true;
  if (depth < MAX_BLOCK_DEPTH && quoteBody(line) !== null) return true;
  const item = itemOf(line);
  return item !== null && (!item.ordered || item.number === 1);
}

function readFence(lines: string[], i: number, ticks: number, lang: string | undefined): Block {
  invariant(i >= 0 && i < lines.length && ticks >= 3, "a fenced block starts at a valid opening fence");
  let end = i + 1;
  while (end < lines.length) {
    const text = (lines[end] ?? "").trim();
    if (text.length >= ticks && backtickRun(text, 0) === text.length) break;
    end++;
  }
  invariant(end > i, "a fenced block spans its opening line");
  const code = lines.slice(i + 1, end).join("\n");
  const node = lang === "mermaid"
    ? <div className="md-diagram-chat"><MermaidDiagram source={code} /><CopyButton text={code} what="the mermaid code" /></div>
    : <CodeBlock code={code} lang={lang} />;
  return { node, next: end + 1 };
}

function CodeBlock({ code, lang }: { readonly code: string; readonly lang: string | undefined }) {
  return (
    <figure className="md-codeblock">
      <figcaption className="md-codebar">
        <span className="md-lang">{lang ?? "text"}</span>
        <CopyButton text={code} what={lang === undefined ? "the code" : `the ${lang} code`} />
      </figcaption>
      <pre className="md-pre"><code data-lang={lang}>{code}</code></pre>
    </figure>
  );
}

/** `$$` on a line of its own opens display math that runs to the next `$$` line. */
function isMathFence(line: string): boolean {
  return line.trim() === "$$";
}

function readMath(lines: string[], i: number): Block {
  let end = i + 1;
  while (end < lines.length && !isMathFence(lines[end] ?? "")) end++;
  invariant(end > i, "display math spans its opening line");
  const tex = lines.slice(i + 1, end).join("\n").trim();
  return { node: <div className="md-math md-math-display" role="math" aria-label={tex} title={tex}>{prettyTex(tex)}</div>, next: end + 1 };
}

// ---------------------------------------------------------------------------------- tables

type Align = "left" | "center" | "right" | undefined;
const DELIMITER_CELL = /^:?-+:?$/;
const MAX_COLUMNS = 64;

/** The cells of a `| a | b |` row; a backslash keeps a pipe inside a cell. */
function cellsOf(line: string): string[] {
  let text = line.trim();
  if (text.startsWith("|")) text = text.slice(1);
  if (text.endsWith("|") && !text.endsWith("\\|")) text = text.slice(0, -1);
  const cells: string[] = [];
  let start = 0;
  for (let p = 0; p < text.length; p++) {
    if (text[p] === "\\") p++;
    else if (text[p] === "|") {
      cells.push(text.slice(start, p).trim());
      start = p + 1;
    }
  }
  cells.push(text.slice(start).trim());
  return cells;
}

function alignments(line: string): Align[] | null {
  if (!line.includes("-")) return null;
  const cells = cellsOf(line);
  if (cells.length > MAX_COLUMNS || !cells.every((cell) => DELIMITER_CELL.test(cell))) return null;
  return cells.map((cell) => (cell.startsWith(":") && cell.endsWith(":") ? "center" : cell.endsWith(":") ? "right" : cell.startsWith(":") ? "left" : undefined));
}

function isTableStart(lines: string[], i: number): boolean {
  const head = lines[i] ?? "";
  if (!head.includes("|")) return false;
  const align = alignments(lines[i + 1] ?? "");
  return align !== null && align.length === cellsOf(head).length;
}

function readTable(lines: string[], i: number): Block {
  const align = alignments(lines[i + 1] ?? "");
  invariant(align !== null, "a table has a delimiter row");
  const head = cellsOf(lines[i] ?? "");
  let end = i + 2;
  const body: string[][] = [];
  for (; end < lines.length && (lines[end] ?? "").includes("|") && !isBlank(lines[end] ?? ""); end++) body.push(cellsOf(lines[end] ?? ""));
  const cell = (text: string | undefined, k: number, Tag: "th" | "td") => <Tag key={k} style={align[k] ? { textAlign: align[k] } : undefined}>{parseInline(text ?? "", 0, true)}</Tag>;
  const node = (
    <div className="md-table-wrap">
      <table className="md-table">
        <thead><tr>{head.map((text, k) => cell(text, k, "th"))}</tr></thead>
        {body.length === 0 ? null : <tbody>{body.map((row, r) => <tr key={r}>{head.map((_, k) => cell(row[k], k, "td"))}</tr>)}</tbody>}
      </table>
    </div>
  );
  return { node, next: end };
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
  while (end < lines.length && !isBlank(lines[end] ?? "") && !startsBlock(lines[end] ?? "", depth) && !isTableStart(lines, end)) end++;
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
  if (isMathFence(line)) return readMath(lines, i);
  if (isTableStart(lines, i)) return readTable(lines, i);
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
