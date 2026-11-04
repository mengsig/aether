// Keyboard focus over a canvas scene: arrows move to the nearest node in that direction,
// Home and End jump to the first and last node in reading order (column, then row).

import { at, invariant } from "./invariant.ts";
import type { Scene, SceneNode } from "./scene.ts";
import type { VisibleNode } from "./model.ts";

export type FocusKey = "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Home" | "End";
const DIRECTIONS: Record<string, readonly [number, number]> = {
  ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1],
};

export function isFocusKey(key: string): key is FocusKey {
  return key in DIRECTIONS || key === "Home" || key === "End";
}

const centre = (n: SceneNode) => ({ x: n.x + n.width / 2, y: n.y + n.height / 2 });

/** Nodes in reading order: by column (x), then row (y), then id. */
export function readingOrder(nodes: readonly SceneNode[]): SceneNode[] {
  return [...nodes].sort((a, b) => a.x - b.x || a.y - b.y || (a.node.id < b.node.id ? -1 : 1));
}

/**
 * The node focus moves to, or `current` when nothing lies that way. Off-axis distance costs
 * double, so a node straight ahead wins over a closer one off to the side.
 */
export function nextFocus(nodes: readonly SceneNode[], current: string | undefined, key: FocusKey): string | undefined {
  if (nodes.length === 0) return undefined;
  const ordered = readingOrder(nodes);
  const from = current === undefined ? undefined : nodes.find((n) => n.node.id === current);
  if (key === "End") return at(ordered, ordered.length - 1).node.id;
  if (key === "Home" || from === undefined) return at(ordered, 0).node.id;
  const direction = DIRECTIONS[key];
  invariant(direction !== undefined, `focus key ${key} has a direction`);
  const [dx, dy] = direction;
  const origin = centre(from);
  let best: SceneNode | undefined;
  let bestCost = Infinity;
  for (const node of nodes) {
    if (node === from) continue;
    const c = centre(node);
    const along = (c.x - origin.x) * dx + (c.y - origin.y) * dy;
    if (along <= 0) continue;
    const across = Math.abs((c.x - origin.x) * dy) + Math.abs((c.y - origin.y) * dx);
    const cost = along + 2 * across;
    if (cost < bestCost) { best = node; bestCost = cost; }
  }
  return (best ?? from).node.id;
}

/** What a screen reader hears for a node: label, then status or group size. */
export function describeNode(node: VisibleNode): string {
  if (node.collapsedMembers !== undefined) {
    return `${node.label}, collapsed group of ${node.collapsedMembers} ${node.collapsedMembers === 1 ? "node" : "nodes"}`;
  }
  return node.status === undefined ? node.label : `${node.label}, ${node.status}`;
}

export type KeyAction =
  | { kind: "focus"; id: string }
  | { kind: "activate"; id: string }
  | { kind: "toggle"; group: string }
  | { kind: "zoom"; factor: number }
  | { kind: "fit" }
  | { kind: "blur" };

const ZOOM_KEYS: Record<string, number> = { "+": 1.25, "=": 1.25, "-": 0.8 };

/**
 * What a key does on the focused graph. Enter activates a node or toggles a group; Escape
 * climbs to the enclosing group, then clears focus.
 */
export function keyAction(scene: Scene, focused: string | undefined, key: string): KeyAction | undefined {
  const node = focused === undefined ? undefined : scene.byId.get(focused);
  const group = focused === undefined ? undefined : scene.groups.find((g) => g.id === focused);
  if (isFocusKey(key)) {
    const id = nextFocus(scene.nodes, node?.node.id, key);
    return id === undefined ? undefined : { kind: "focus", id };
  }
  if (key === "Enter" || key === " ") {
    if (group !== undefined) return { kind: "toggle", group: group.id };
    if (node === undefined) return undefined;
    return node.node.collapsedMembers !== undefined ? { kind: "toggle", group: node.node.id } : { kind: "activate", id: node.node.id };
  }
  if (key === "Escape") {
    const parent = node?.node.group ?? group?.parent;
    return parent !== undefined && scene.groups.some((g) => g.id === parent) ? { kind: "focus", id: parent } : { kind: "blur" };
  }
  const factor = ZOOM_KEYS[key];
  if (factor !== undefined) return { kind: "zoom", factor };
  return key === "0" ? { kind: "fit" } : undefined;
}

/** The live-region text for the focused node or group. */
export function announce(scene: Scene, focused: string | undefined): string {
  if (focused === undefined) return "";
  const node = scene.byId.get(focused);
  if (node !== undefined) return describeNode(node.node);
  const group = scene.groups.find((g) => g.id === focused);
  return group === undefined ? "" : `${group.label}, expanded group`;
}
