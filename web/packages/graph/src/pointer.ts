import { useEffect, useRef, type RefObject } from "react";
import { invariant } from "./invariant.ts";
import type { Hit } from "./scene.ts";
import type { Surface } from "./surface.ts";

function local(canvas: HTMLCanvasElement, event: { clientX: number; clientY: number }): [number, number] {
  const box = canvas.getBoundingClientRect();
  return [event.clientX - box.left, event.clientY - box.top];
}

interface Pointer { x: number; y: number }

class Gesture {
  private readonly pointers = new Map<number, Pointer>();
  private moved = false;
  private readonly canvas: HTMLCanvasElement;
  private readonly surface: RefObject<Surface | null>;
  constructor(canvas: HTMLCanvasElement, surface: RefObject<Surface | null>) { this.canvas = canvas; this.surface = surface; }

  down(event: PointerEvent): void {
    if (event.button !== 0) return;
    if (this.pointers.size === 0) this.moved = false;
    this.pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    this.canvas.setPointerCapture(event.pointerId);
  }

  move(event: PointerEvent): boolean {
    const point = this.pointers.get(event.pointerId), s = this.surface.current;
    if (point === undefined || s === null) return false;
    const dx = event.clientX - point.x, dy = event.clientY - point.y;
    if (!this.moved && this.pointers.size === 1 && Math.hypot(dx, dy) < 3) return true;
    this.moved = true;
    const other = this.pointers.size === 2 ? [...this.pointers.values()].find((p) => p !== point) : undefined;
    if (other !== undefined) this.pinch(s, point, other, event);
    else s.panBy(dx, dy);
    point.x = event.clientX; point.y = event.clientY;
    return true;
  }

  private pinch(s: Surface, point: Pointer, other: Pointer, event: PointerEvent): void {
    const before = Math.hypot(point.x - other.x, point.y - other.y);
    const after = Math.hypot(event.clientX - other.x, event.clientY - other.y);
    if (before < 1 || after < 1) return;
    invariant(Number.isFinite(before) && Number.isFinite(after), "pinch distances are finite");
    invariant(this.pointers.size === 2, "a pinch tracks two pointers");
    const [x, y] = local(this.canvas, { clientX: (point.x + other.x) / 2, clientY: (point.y + other.y) / 2 });
    s.zoomAt(after / before, x, y);
    s.panBy((event.clientX - point.x) / 2, (event.clientY - point.y) / 2);
  }

  up(event: PointerEvent): boolean {
    const click = this.pointers.has(event.pointerId) && !this.moved && this.pointers.size === 1;
    this.pointers.delete(event.pointerId);
    return click;
  }

  cancel(id: number): void { this.pointers.delete(id); }
}

/** Drag pans, two pointers pinch, the wheel zooms at the pointer, and a click without a drag hits. */
export function usePointer(canvasRef: RefObject<HTMLCanvasElement | null>, surface: RefObject<Surface | null>, onHit: (hit: Hit | undefined) => void, onHover: (hit: Hit | undefined) => void): void {
  const onHitRef = useRef(onHit), onHoverRef = useRef(onHover);
  useEffect(() => { onHitRef.current = onHit; onHoverRef.current = onHover; });
  useEffect(() => {
    const canvas = canvasRef.current;
    invariant(canvas !== null, "GraphCanvas mounts its canvas before its effects run");
    const gesture = new Gesture(canvas, surface);
    const down = (e: PointerEvent) => gesture.down(e);
    const move = (e: PointerEvent) => {
      const s = surface.current;
      if (s === null || gesture.move(e)) return;
      const hit = s.hitAt(...local(canvas, e));
      s.renderer.update({ hovered: hit?.id }); onHoverRef.current(hit);
    };
    const up = (e: PointerEvent) => { if (gesture.up(e)) onHitRef.current(surface.current?.hitAt(...local(canvas, e))); };
    const cancel = (e: PointerEvent) => gesture.cancel(e.pointerId);
    const leave = () => { surface.current?.renderer.update({ hovered: undefined }); onHoverRef.current(undefined); };
    const doubleClick = (e: MouseEvent) => surface.current?.zoomAt(1.5, ...local(canvas, e));
    const wheel = (e: WheelEvent) => {
      if (surface.current === null) return;
      e.preventDefault();
      const pixels = e.deltaY * (e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? surface.current.height : 1);
      surface.current.zoomAt(Math.exp(Math.max(-2, Math.min(2, -pixels * 0.002))), ...local(canvas, e));
    };
    const listeners = [["pointerdown", down], ["pointermove", move], ["pointerup", up], ["pointercancel", cancel], ["pointerleave", leave], ["lostpointercapture", cancel]] as const;
    for (const [type, listener] of listeners) canvas.addEventListener(type, listener);
    canvas.addEventListener("wheel", wheel, { passive: false });
    canvas.addEventListener("dblclick", doubleClick);
    return () => {
      for (const [type, listener] of listeners) canvas.removeEventListener(type, listener);
      canvas.removeEventListener("wheel", wheel);
      canvas.removeEventListener("dblclick", doubleClick);
    };
  }, [canvasRef, surface]);
}
