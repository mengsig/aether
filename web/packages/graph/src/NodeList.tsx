// The screen-reader node list beside the canvas: one button per visible node and expanded group,
// in reading order. Buttons are reachable in browse mode; the canvas itself holds the tab stop.

import type { CSSProperties, ReactNode } from "react";
import { describeNode } from "./focus.ts";
import type { VisibleGroup, VisibleNode } from "./model.ts";

export interface GraphNodeListProps {
  label: string;
  nodes: readonly VisibleNode[];
  groups: readonly VisibleGroup[];
  focused: string | undefined;
  onFocusNode: (id: string) => void;
  onActivate: ((id: string) => void) | undefined;
  onToggleGroup: ((group: string) => void) | undefined;
}

export const VISUALLY_HIDDEN: CSSProperties = {
  position: "absolute", width: 1, height: 1, margin: -1, padding: 0, border: 0,
  overflow: "hidden", clip: "rect(0 0 0 0)", whiteSpace: "nowrap",
};

export function GraphNodeList(props: GraphNodeListProps): ReactNode {
  const { nodes, groups, focused } = props;
  const count = `${nodes.length} ${nodes.length === 1 ? "node" : "nodes"}`;
  return (
    <ul aria-label={`${props.label}: ${count}`} style={VISUALLY_HIDDEN}>
      {groups.map((group) => (
        <li key={group.id}>
          <button type="button" tabIndex={-1} aria-expanded={true} aria-current={focused === group.id ? true : undefined}
            onFocus={() => props.onFocusNode(group.id)} onClick={() => props.onToggleGroup?.(group.id)}>
            {`Group ${group.label}`}
          </button>
        </li>
      ))}
      {nodes.map((node) => {
        const collapsed = node.collapsedMembers !== undefined;
        return (
          <li key={node.id}>
            <button type="button" tabIndex={-1} aria-expanded={collapsed ? false : undefined}
              aria-current={focused === node.id ? true : undefined} onFocus={() => props.onFocusNode(node.id)}
              onClick={() => (collapsed ? props.onToggleGroup?.(node.id) : props.onActivate?.(node.id))}>
              {describeNode(node)}
            </button>
          </li>
        );
      })}
    </ul>
  );
}
