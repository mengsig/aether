import type { GNode } from "@aether-ui/protocol";
import { useEffect, useState, type KeyboardEvent, type RefObject } from "react";
import { keyAction, type KeyAction } from "./focus.ts";
import type { GraphCanvasProps } from "./GraphCanvas.tsx";
import { usePointer } from "./pointer.ts";
import type { Scene } from "./scene.ts";
import type { Surface } from "./surface.ts";

export function useInteraction(props: GraphCanvasProps, scene: Scene, canvasRef: RefObject<HTMLCanvasElement | null>, surface: RefObject<Surface | null>) {
  const { selected, emphasis, onActivate, onToggleGroup, onSelect, onHover } = props;
  const [focused, setFocused] = useState<string>();
  const [hovered, setHovered] = useState<string>();
  const nodeAt = (id: string | undefined): GNode | null => id === undefined ? null : scene.byId.get(id)?.node ?? null;
  const focus = (id: string | undefined) => { setFocused(id); onSelect?.(nodeAt(id)); };
  const activate = (id: string) => { focus(id); onActivate?.(id); };
  useEffect(() => { surface.current?.renderer.update({ selected }); }, [surface, selected]);
  useEffect(() => { surface.current?.renderer.update({ emphasis }); }, [surface, emphasis]);
  useEffect(() => {
    surface.current?.renderer.update({ focused });
    if (focused !== undefined) surface.current?.reveal(focused);
  }, [surface, focused]);
  usePointer(canvasRef, surface, (hit) => {
    if (hit === undefined) { focus(undefined); return; }
    if (hit.kind === "group" || scene.byId.get(hit.id)?.node.collapsedMembers !== undefined) {
      focus(hit.id); onToggleGroup?.(hit.id);
    } else activate(hit.id);
  }, (hit) => {
    const id = hit?.kind === "node" ? hit.id : undefined;
    if (id === hovered) return;
    setHovered(id); onHover?.(nodeAt(id));
  });
  const onKeyDown = (event: KeyboardEvent<HTMLCanvasElement>) => {
    const action = keyAction(scene, focused, event.key);
    if (action === undefined || event.altKey || event.ctrlKey || event.metaKey) return;
    event.preventDefault();
    perform(action, { surface: surface.current, focus, activate, onToggleGroup });
  };
  return { focused, hovered, focus, activate, onKeyDown };
}

interface Handlers {
  surface: Surface | null;
  focus: (id: string | undefined) => void;
  activate: (id: string) => void;
  onToggleGroup: ((group: string) => void) | undefined;
}

function perform(action: KeyAction, h: Handlers): void {
  switch (action.kind) {
    case "focus": return h.focus(action.id);
    case "blur": return h.focus(undefined);
    case "activate": return h.activate(action.id);
    case "toggle": return h.onToggleGroup?.(action.group);
    case "zoom": return h.surface?.zoomAt(action.factor);
    case "fit": return h.surface?.fit();
  }
}
