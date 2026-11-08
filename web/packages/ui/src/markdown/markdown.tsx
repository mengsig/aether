import { useMemo, type ReactNode } from "react";
import { invariant } from "../classes.ts";
import { parseBlocks } from "./blocks.tsx";
import { parseInline } from "./inline.tsx";
import "./markdown.css";

export function Markdown(props: { readonly text: string }): ReactNode {
  invariant(typeof props.text === "string", "Markdown receives text");
  const blocks = useMemo(() => parseBlocks(props.text.replace(/\r\n?/g, "\n").split("\n"), 0), [props.text]);
  return <div className="md">{blocks}</div>;
}

/** Phrasing content only, suitable inside labels, summaries and answer keys. */
export function InlineMarkdown({ text }: { readonly text: string }): ReactNode {
  invariant(typeof text === "string", "InlineMarkdown receives text");
  const content = useMemo(() => parseInline(text.replace(/\r?\n/g, " "), 0, true), [text]);
  return <span className="md-inline">{content}</span>;
}
