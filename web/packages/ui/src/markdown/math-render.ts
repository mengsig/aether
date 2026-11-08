import katex from "katex";
import { invariant } from "../classes.ts";

export const MAX_MATH_LENGTH = 8192;

export function renderMath(source: string, display: boolean): string {
  invariant(source.length > 0 && source.length <= MAX_MATH_LENGTH, "math has bounded, nonempty source");
  const markup = katex.renderToString(source, {
    displayMode: display, output: "mathml", throwOnError: false, trust: false, strict: "error",
    maxExpand: 200, maxSize: 20, errorColor: "currentColor",
  });
  invariant(markup.startsWith("<span"), "KaTeX returns a wrapped MathML expression or escaped error");
  return markup;
}
