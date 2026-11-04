// @aether-ui/graph: Canvas 2D graph rendering, viewport, worker layouts and the graph model
// (CONTRACTS.md §2.1). Data arrives as props; nothing here talks to a transport.

export type { GEdge, GGroup, GNode, GraphModel, Layout } from "@aether-ui/protocol";
export { GraphCanvas, type GraphCanvasProps } from "./GraphCanvas.tsx";
export { GraphNodeList, type GraphNodeListProps } from "./NodeList.tsx";
export { Viewport, toWorld, visibleWorld, MAX_SCALE, MIN_SCALE, type Camera, type Rect } from "./viewport.ts";
export { LayoutEngine, type LayoutEngineOptions, type LayoutStats, type LayoutWorker } from "./engine.ts";
export { browserLayoutWorker, forceLayout, layeredLayout, sharedLayoutEngine } from "./browser.ts";
export { GraphModelError, structureHash, toggleCollapsed, visibleGraph,
  type VisibleEdge, type VisibleGraph, type VisibleGroup, type VisibleNode } from "./model.ts";
export { buildScene, hitTest, type Hit, type NodeShape, type Scene, type SceneGroup, type SceneNode } from "./scene.ts";
export { edgeKey, GraphRenderer, type Emphasis, type FrameScheduler, type GraphTheme } from "./renderer.ts";
export { describeNode, keyAction, nextFocus, type KeyAction } from "./focus.ts";
