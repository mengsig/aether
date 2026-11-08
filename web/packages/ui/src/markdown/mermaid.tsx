import { useEffect, useId, useRef, useState } from "react";
import { useTheme } from "../theme.tsx";
import { CodeBlock } from "./code.tsx";
import { diagramRefusal } from "./mermaid-policy.ts";
import "./mermaid.css";

interface DiagramResult { readonly source: string; readonly theme: string | undefined; readonly image?: string; readonly error?: string }

export function MermaidDiagram({ source }: { readonly source: string }) {
  const id = `diagram-${useId().replace(/[^\w-]/g, "")}`;
  const theme = useTheme()?.resolved;
  const ref = useRef<HTMLElement>(null);
  const [result, setResult] = useState<DiagramResult | null>(null);
  const refusal = diagramRefusal(source);
  useEffect(() => {
    const element = ref.current;
    if (refusal !== null || element === null) return;
    let active = true;
    const controller = new AbortController();
    void import("./mermaid-render.ts").then(({ renderDiagram, diagramPalette }) => {
      if (!active) return undefined;
      return renderDiagram(source, id, diagramPalette(element), controller.signal);
    }).then(
      (image) => { if (active && image !== undefined) setResult({ source, theme, image }); },
      (error: unknown) => { if (active) setResult({ source, theme, error: `Diagram unavailable: ${String(error)}` }); },
    );
    return () => { active = false; controller.abort(); };
  }, [source, id, theme, refusal]);
  const current = result?.source === source && result.theme === theme ? result : null;
  const note = refusal ?? current?.error;
  return <figure className="md-diagram" ref={ref} data-theme={theme}>
    {current?.image ? <img className="md-diagram-image" src={current.image} alt="Mermaid diagram; source available below" /> : <CodeBlock source={source} language="mermaid" />}
    {note && <figcaption className="md-diagram-note">{note}</figcaption>}
    {current?.image && <details><summary>Diagram source</summary><CodeBlock source={source} language="mermaid" /></details>}
  </figure>;
}
