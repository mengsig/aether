import { useEffect, useState } from "react";

interface Result { readonly source: string; readonly display: boolean; readonly markup?: string; readonly error?: string }

export function MathExpression({ source, display = false }: { readonly source: string; readonly display?: boolean }) {
  const [result, setResult] = useState<Result | null>(null);
  useEffect(() => {
    let active = true;
    void import("./math-render.ts").then(({ renderMath }) => renderMath(source, display)).then(
      (markup) => { if (active) setResult({ source, display, markup }); },
      (error: unknown) => { if (active) setResult({ source, display, error: `Math unavailable: ${String(error)}` }); },
    );
    return () => { active = false; };
  }, [source, display]);
  const current = result?.source === source && result.display === display ? result : null;
  const className = display ? "md-math md-math-display" : "md-math";
  if (current?.markup === undefined) return <span className={className} title={current?.error}>{display ? "$$" : "$"}{source}{display ? "$$" : "$"}</span>;
  // This is exclusively KaTeX output: trust:false disables URL/HTML commands and KaTeX escapes input.
  // Native MathML needs no vendor CSS, fonts, image requests or HTML from the lesson.
  return <span className={className} dangerouslySetInnerHTML={{ __html: current.markup }} />;
}
