import { invariant } from "../classes.ts";
import { diagramRefusal } from "./mermaid-policy.ts";

export interface DiagramPalette { readonly surface: string; readonly text: string; readonly border: string; readonly accent: string; readonly font: string }

export function diagramPalette(element: HTMLElement): DiagramPalette {
  invariant(element.isConnected, "diagram colors come from a mounted theme");
  const style = getComputedStyle(element);
  const token = (name: string) => style.getPropertyValue(name).trim();
  const palette = { surface: token("--color-surface-1"), text: token("--color-text"), border: token("--color-border-strong"), accent: token("--color-accent"), font: token("--font-ui") };
  invariant(Object.values(palette).every((value) => value !== ""), "the diagram has every required theme token");
  return palette;
}

const TIMEOUT_MS = 30_000;
let frame: HTMLIFrameElement | undefined;
let ready: Promise<HTMLIFrameElement> | undefined;
let rendering: Promise<unknown> = Promise.resolve();
let requestId = 0;

function resetFrame(): void {
  frame?.remove();
  frame = undefined;
  ready = undefined;
}

function rendererFrame(): Promise<HTMLIFrameElement> {
  if (ready !== undefined) return ready;
  invariant(document.body !== null && document.body.isConnected, "the diagram frame needs a mounted document");
  const element = document.createElement("iframe");
  element.className = "md-diagram-frame";
  element.setAttribute("aria-hidden", "true");
  element.setAttribute("tabindex", "-1");
  element.setAttribute("sandbox", "allow-scripts allow-same-origin");
  const url = new URL("mermaid-frame.html", document.baseURI);
  invariant(url.protocol === location.protocol && url.host === location.host, "the diagram frame stays on the app origin");
  element.src = url.href;
  frame = element;
  ready = new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("Diagram renderer failed to load")), TIMEOUT_MS);
    element.addEventListener("load", () => { clearTimeout(timeout); resolve(element); }, { once: true });
    element.addEventListener("error", () => { clearTimeout(timeout); reject(new Error("Diagram renderer failed to load")); }, { once: true });
    document.body.append(element);
  });
  return ready;
}

function requestDiagram(element: HTMLIFrameElement, source: string, id: string, palette: DiagramPalette): Promise<string> {
  const target = element.contentWindow;
  invariant(target !== null, "the diagram frame has a window");
  const request = ++requestId;
  invariant(Number.isSafeInteger(request), "diagram request identifiers remain unique");
  return new Promise((resolve, reject) => {
    const finish = (error?: Error, image?: string, reset = false) => {
      clearTimeout(timeout);
      window.removeEventListener("message", receive);
      if (error !== undefined) { if (reset) resetFrame(); reject(error); }
      else if (image !== undefined) resolve(image);
      else reject(new Error("Diagram renderer returned no image"));
    };
    const receive = (event: MessageEvent<unknown>) => {
      if (event.source !== target || typeof event.data !== "object" || event.data === null) return;
      const reply = event.data as { request?: unknown; image?: unknown; error?: unknown };
      if (reply.request !== request) return;
      if (typeof reply.image === "string" && reply.image.startsWith("data:image/svg+xml;charset=utf-8,") && reply.image.length <= 18_000_100) finish(undefined, reply.image);
      else if (typeof reply.error === "string") finish(new Error(reply.error));
      else finish(new Error("Invalid diagram renderer response"), undefined, true);
    };
    const timeout = setTimeout(() => finish(new Error("Diagram renderer timed out"), undefined, true), TIMEOUT_MS);
    window.addEventListener("message", receive);
    target.postMessage({ request, source, id, palette }, "*");
  });
}

async function renderOne(source: string, id: string, palette: DiagramPalette): Promise<string> {
  const element = await rendererFrame().catch((error: unknown) => { resetFrame(); throw error; });
  return requestDiagram(element, source, id, palette);
}

export function renderDiagram(source: string, id: string, palette: DiagramPalette, signal?: AbortSignal): Promise<string> {
  invariant(diagramRefusal(source) === null, "only approved diagram source reaches Mermaid");
  invariant(/^[a-z][\w-]+$/.test(id), "diagram IDs are generated identifiers");
  const result = rendering.then(() => {
    if (signal?.aborted) throw new DOMException("Diagram rendering cancelled", "AbortError");
    return renderOne(source, id, palette);
  });
  rendering = result.then(() => undefined, () => undefined);
  return result;
}
