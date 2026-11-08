import type { Context2D, GraphTheme } from "./renderer.ts";
import type { SceneEdge, SceneNode } from "./scene.ts";
import type { Rect } from "./viewport.ts";

const SOLID: number[] = [];
const DASHED = [6, 4];
const DOTTED = [2, 4];

function outline(ctx: Context2D, rect: Rect, radius: number): void {
  const { x, y, width: w, height: h } = rect;
  const r = Math.min(radius, w / 2, h / 2), c = r * 0.55228475;
  ctx.moveTo(x + r, y);
  ctx.lineTo(x + w - r, y);
  ctx.bezierCurveTo(x + w - r + c, y, x + w, y + r - c, x + w, y + r);
  ctx.lineTo(x + w, y + h - r);
  ctx.bezierCurveTo(x + w, y + h - r + c, x + w - r + c, y + h, x + w - r, y + h);
  ctx.lineTo(x + r, y + h);
  ctx.bezierCurveTo(x + r - c, y + h, x, y + h - r + c, x, y + h - r);
  ctx.lineTo(x, y + r);
  ctx.bezierCurveTo(x, y + r - c, x + r - c, y, x + r, y);
}

export function paintShape(ctx: Context2D, node: SceneNode): void {
  if (ctx.fill === undefined) {
    ctx.fillRect(node.x, node.y, node.width, node.height);
    ctx.strokeRect(node.x, node.y, node.width, node.height);
    return;
  }
  ctx.beginPath();
  const shape = node.style?.shape;
  if (shape === "diamond") {
    const { x, y, width: w, height: h } = node;
    ctx.moveTo(x + w / 2, y); ctx.lineTo(x + w, y + h / 2);
    ctx.lineTo(x + w / 2, y + h); ctx.lineTo(x, y + h / 2); ctx.lineTo(x + w / 2, y);
  } else outline(ctx, node, shape === "circle" || shape === "pill" ? node.height / 2 : 8);
  ctx.fill();
  const alpha = ctx.globalAlpha ?? 1;
  ctx.fillStyle = ctx.strokeStyle;
  ctx.globalAlpha = alpha * 0.07;
  ctx.fill();
  ctx.globalAlpha = alpha;
  ctx.stroke();
}

export function paintLabel(ctx: Context2D, theme: GraphTheme, text: string, x: number, y: number, colour = theme.text): void {
  ctx.strokeStyle = theme.surface;
  ctx.lineWidth = 4;
  ctx.strokeText?.(text, x, y);
  ctx.fillStyle = colour;
  ctx.fillText(text, x, y);
}

export function paintEdge(ctx: Context2D, edge: SceneEdge, scale: number, strong = false): void {
  const kind = edge.kind.toLowerCase();
  const dash = strong || kind === "initiative_after" ? SOLID : /link|reference|related|stacked/.test(kind) ? DOTTED
    : /depend|wait|after|requires/.test(kind) ? DASHED : SOLID;
  ctx.setLineDash?.(dash);
  const [a, b, c, d] = edge.curve;
  ctx.beginPath();
  ctx.moveTo(a.x, a.y);
  ctx.bezierCurveTo(b.x, b.y, c.x, c.y, d.x, d.y);
  ctx.stroke();
  ctx.setLineDash?.(SOLID);
  const angle = Math.atan2(d.y - c.y || d.y - a.y, d.x - c.x || d.x - a.x);
  const size = 5 / Math.max(0.6, scale);
  ctx.beginPath();
  ctx.moveTo(d.x - Math.cos(angle - 0.45) * size, d.y - Math.sin(angle - 0.45) * size);
  ctx.lineTo(d.x, d.y);
  ctx.lineTo(d.x - Math.cos(angle + 0.45) * size, d.y - Math.sin(angle + 0.45) * size);
  ctx.stroke();
}
