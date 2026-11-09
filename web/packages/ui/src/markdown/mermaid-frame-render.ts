import mermaid from "mermaid";
import { invariant } from "../classes.ts";
import { diagramRefusal, MAX_DIAGRAM_LENGTH } from "./mermaid-policy.ts";

import type { DiagramPalette } from "./mermaid-render.ts";

// Mermaid's configuration is global; serialize initialization with rendering to prevent theme races.
let rendering: Promise<unknown> = Promise.resolve();

export function renderDiagram(source: string, id: string, palette: DiagramPalette): Promise<string> {
  invariant(diagramRefusal(source) === null, "only approved diagram source reaches Mermaid");
  invariant(/^[a-z][\w-]+$/.test(id), "diagram IDs are generated identifiers");
  const result = rendering.then(() => renderOne(source, id, palette));
  // Release the queue after failure; the original rejected result still reaches the caller.
  rendering = result.then(() => undefined, () => undefined);
  return result;
}

async function renderOne(source: string, id: string, palette: DiagramPalette): Promise<string> {
  const host = document.createElement("div");
  host.className = "md-diagram-measure";
  host.setAttribute("aria-hidden", "true");
  document.body.append(host);
  try {
    mermaid.initialize({
      startOnLoad: false, securityLevel: "strict", htmlLabels: false, suppressErrorRendering: true,
      maxTextSize: MAX_DIAGRAM_LENGTH, maxEdges: 200, theme: "base",
      secure: ["secure", "securityLevel", "startOnLoad", "htmlLabels", "themeCSS", "themeVariables", "maxTextSize", "maxEdges", "suppressErrorRendering"],
      themeVariables: { primaryColor: palette.surface, primaryTextColor: palette.text, primaryBorderColor: palette.border,
        lineColor: palette.accent, secondaryColor: palette.surface, tertiaryColor: palette.surface, fontFamily: palette.font,
        background: palette.surface, textColor: palette.text, mainBkg: palette.surface, nodeBorder: palette.border,
        edgeLabelBackground: palette.surface, noteBkgColor: palette.surface, noteTextColor: palette.text, noteBorderColor: palette.border,
        actorBkg: palette.surface, actorTextColor: palette.text, actorBorder: palette.border, actorLineColor: palette.border,
        signalColor: palette.text, signalTextColor: palette.text, labelBoxBkgColor: palette.surface, labelBoxBorderColor: palette.border,
        labelTextColor: palette.text, loopTextColor: palette.text, activationBkgColor: palette.surface, activationBorderColor: palette.border },
      flowchart: { htmlLabels: false },
    });
    const { svg } = await mermaid.render(id, source, host);
    invariant(svg.includes("<svg"), "Mermaid returned an SVG document");
    invariant(svg.length <= 2_000_000, "diagram output is bounded");
    // SVG image documents cannot execute scripts or load external resources; no bindFunctions call.
    return diagramImage(svg);
  } finally {
    host.remove();
  }
}

function diagramImage(svg: string): string {
  const document = new DOMParser().parseFromString(svg, "image/svg+xml");
  const root = document.documentElement;
  invariant(root.localName === "svg" && document.querySelector("parsererror") === null, "diagram output is valid SVG");
  const unsafe = [...root.querySelectorAll("*")].filter((element) => ["script", "foreignobject"].includes(element.localName.toLowerCase()));
  unsafe.forEach((element) => element.remove());
  invariant(unsafe.every((element) => !root.contains(element)), "diagram images contain no active SVG or HTML");
  const box = root.getAttribute("viewBox")?.trim().split(/[\s,]+/).map(Number);
  if (box !== undefined) {
    const width = box[2] ?? 0;
    const height = box[3] ?? 0;
    invariant(box.length === 4 && box.every(Number.isFinite) && width > 0 && height > 0, "diagram viewBox is finite and nonempty");
    invariant(width <= 100_000 && height <= 100_000, "diagram image dimensions are bounded");
    root.setAttribute("width", String(width));
    root.setAttribute("height", String(height));
  }
  const sized = new XMLSerializer().serializeToString(root);
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(sized)}`;
}
