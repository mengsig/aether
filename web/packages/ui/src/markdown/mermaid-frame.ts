import { renderDiagram } from "./mermaid-frame-render.ts";
import type { DiagramPalette } from "./mermaid-render.ts";

interface Request { readonly request: number; readonly source: string; readonly id: string; readonly palette: DiagramPalette }

function isRequest(value: unknown): value is Request {
  if (typeof value !== "object" || value === null) return false;
  const request = value as Record<string, unknown>;
  if (typeof request.request !== "number" || !Number.isSafeInteger(request.request) || request.request < 1) return false;
  if (typeof request.source !== "string" || typeof request.id !== "string") return false;
  if (typeof request.palette !== "object" || request.palette === null) return false;
  const palette = request.palette as Record<string, unknown>;
  return ["surface", "text", "border", "accent", "font"].every((key) => typeof palette[key] === "string" && palette[key] !== "");
}

window.addEventListener("message", (event: MessageEvent<unknown>) => {
  if (event.source !== window.parent || !isRequest(event.data)) return;
  const { request, source, id, palette } = event.data;
  void renderDiagram(source, id, palette).then(
    (image) => window.parent.postMessage({ request, image }, "*"),
    (error: unknown) => window.parent.postMessage({ request, error: String(error) }, "*"),
  );
});
