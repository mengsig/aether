import { Markdown } from "@aether-ui/ui";
import type { ReactNode } from "react";

type ObjectValue = Record<string, unknown>;
const object = (value: unknown): value is ObjectValue => value !== null && typeof value === "object" && !Array.isArray(value);
const label = (key: string) => key.replaceAll("_", " ").replace(/^./, (first) => first.toUpperCase());

function Table({ rows, depth }: { readonly rows: readonly ObjectValue[]; readonly depth: number }) {
  const columns = [...new Set(rows.flatMap((row) => Object.keys(row)))];
  return <div className="pipeline-table"><table><thead><tr>{columns.map((key) => <th key={key}>{label(key)}</th>)}</tr></thead>
    <tbody>{rows.map((row, index) => <tr key={index}>{columns.map((key) => <td key={key}><Value value={row[key]} depth={depth + 1} /></td>)}</tr>)}</tbody></table></div>;
}

function Value({ value, depth }: { readonly value: unknown; readonly depth: number }): ReactNode {
  if (value == null) return null;
  if (typeof value === "string") return <Markdown text={value} />;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  if (depth >= 4) return "Details omitted";
  if (Array.isArray(value)) {
    if (value.length > 0 && value.every(object)) return <Table rows={value} depth={depth + 1} />;
    return <ul>{value.map((entry, index) => <li key={index}><Value value={entry} depth={depth + 1} /></li>)}</ul>;
  }
  if (!object(value)) return null;
  return <dl>{Object.entries(value).map(([key, entry]) => <div key={key}><dt>{label(key)}</dt><dd><Value value={entry} depth={depth + 1} /></dd></div>)}</dl>;
}

export function StageResult({ text }: { readonly text: string }) {
  let value: unknown = text;
  if (text.trimStart().startsWith("{")) {
    try { value = JSON.parse(text) as unknown; } catch (error) { if (!(error instanceof SyntaxError)) throw error; }
  }
  return <div className="pipeline-summary">{object(value) ? <Value value={value} depth={0} /> : <Markdown text={text} />}</div>;
}
