import type { ReactNode } from "react";
import { invariant } from "../classes.ts";

const KEYWORDS = new Set("as async await break case catch class const continue def delete do else enum export extends false finally fn for from function if impl import in interface let match new None null pass pub raise return self static struct super switch this throw true try type typeof undefined use var void while with yield".split(" "));
// Unclosed strings/comments consume the remainder, so repeated failed openers cannot scan quadratically.
// Token text is always a React child, including code that looks like HTML.
const TOKENS = /("(?:\\[\s\S]|[^"\\])*(?:"|\\?$)|'(?:\\[\s\S]|[^'\\])*(?:'|\\?$)|`(?:\\[\s\S]|[^`\\])*(?:`|\\?$))|(\/\/[^\n]*|#[^\n]*|\/\*[\s\S]*?(?:\*\/|$))|(\b\d+(?:\.\d+)?\b)|(\b[A-Za-z_]\w*\b)/g;

function highlighted(source: string): ReactNode[] {
  invariant(typeof source === "string", "code source is text");
  const nodes: ReactNode[] = [];
  let end = 0;
  for (const match of source.matchAll(TOKENS)) {
    invariant(match.index >= end && match[0].length > 0, "code tokens consume input without overlap");
    if (match.index > end) nodes.push(source.slice(end, match.index));
    const kind = match[1] ? "string" : match[2] ? "comment" : match[3] ? "number" : KEYWORDS.has(match[0]) ? "keyword" : null;
    nodes.push(kind === null ? match[0] : <span key={match.index} className={`md-token-${kind}`}>{match[0]}</span>);
    end = match.index + match[0].length;
  }
  if (end < source.length) nodes.push(source.slice(end));
  return nodes;
}

export function CodeBlock({ source, language }: { readonly source: string; readonly language?: string | undefined }) {
  return <pre className="md-pre"><code data-lang={language} className={language ? `language-${language}` : undefined}>{highlighted(source)}</code></pre>;
}
