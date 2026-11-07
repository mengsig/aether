// A small CSS reader for the kit's own stylesheets in tests: rule blocks with the
// at-rules around them. It handles nesting by braces and skips comments, no more.

export interface CssBlock {
  readonly selector: string;
  /** Enclosing at-rules, outermost first, e.g. ["@media (prefers-reduced-motion: reduce)"]. */
  readonly within: readonly string[];
  readonly declarations: ReadonlyMap<string, string>;
}

function declarations(body: string): Map<string, string> {
  const found = new Map<string, string>();
  for (const part of body.split(";")) {
    const colon = part.indexOf(":");
    if (colon <= 0) continue;
    found.set(part.slice(0, colon).trim(), part.slice(colon + 1).trim());
  }
  return found;
}

export function cssBlocks(css: string): CssBlock[] {
  const text = css.replace(/\/\*[\s\S]*?\*\//g, "");
  const blocks: CssBlock[] = [];
  const stack: string[] = [];
  let start = 0;
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char === "{") {
      const head = text.slice(start, index).split(";").pop()?.trim() ?? "";
      if (head.startsWith("@")) {
        stack.push(head);
        start = index + 1;
        continue;
      }
      const close = text.indexOf("}", index);
      if (close < 0) throw new Error(`unclosed block after ${head}`);
      blocks.push({ selector: head, within: [...stack], declarations: declarations(text.slice(index + 1, close)) });
      index = close;
      start = close + 1;
    } else if (char === "}") {
      if (stack.pop() === undefined) throw new Error("unbalanced } in CSS");
      start = index + 1;
    }
  }
  if (stack.length > 0) throw new Error(`unclosed ${stack.join(", ")}`);
  return blocks;
}
