import type { ReactNode } from "react";
import { invariant } from "../classes.ts";
import { parseInline } from "./inline.tsx";

function cells(line: string): string[] {
  invariant(typeof line === "string", "table cells come from text");
  invariant(!line.includes("\n"), "a table row occupies one source line");
  const text = line.trim().replace(/^\|/, "").replace(/(?<!\\)\|$/, "");
  const out: string[] = [];
  let start = 0;
  let code = false;
  for (let i = 0; i < text.length; i++) {
    if (text[i] === "\\") { i++; continue; }
    if (text[i] === "`") code = !code;
    if (text[i] !== "|" || code) continue;
    out.push(text.slice(start, i).trim());
    start = i + 1;
  }
  out.push(text.slice(start).trim());
  return out;
}

export function tableAt(lines: readonly string[], i: number): boolean {
  invariant(Array.isArray(lines), "table input is a line buffer");
  invariant(Number.isInteger(i) && i >= 0, "a table probe has a valid line index");
  const heading = lines[i] ?? "";
  const divider = lines[i + 1] ?? "";
  if (!heading.includes("|") || !divider.includes("-")) return false;
  const columns = cells(divider);
  return columns.length === cells(heading).length && columns.every((cell) => /^:?-{3,}:?$/.test(cell));
}

export function readTable(lines: readonly string[], i: number): { node: ReactNode; next: number } {
  invariant(tableAt(lines, i), "a table has matching headers and dividers");
  const headings = cells(lines[i] ?? "");
  const align = cells(lines[i + 1] ?? "").map((cell) => cell.endsWith(":") ? (cell.startsWith(":") ? "center" : "right") : "left");
  const rows: string[][] = [];
  let next = i + 2;
  while (next < lines.length && (lines[next] ?? "").includes("|") && (lines[next] ?? "").trim() !== "") {
    rows.push(cells(lines[next] ?? ""));
    next++;
  }
  invariant(next > i + 1 && headings.length > 0, "a table consumes its header and divider");
  return {
    node: <div className="md-table-scroll"><table>
      <thead><tr>{headings.map((heading, column) => <th key={column} scope="col" data-align={align[column]}>{parseInline(heading, 0, true)}</th>)}</tr></thead>
      <tbody>{rows.map((row, index) => <tr key={index}>{headings.map((_, column) => <td key={column} data-align={align[column]}>{parseInline(row[column] ?? "", 0, true)}</td>)}</tr>)}</tbody>
    </table></div>, next,
  };
}
