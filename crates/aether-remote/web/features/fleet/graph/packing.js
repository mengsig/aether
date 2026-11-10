import { invariant } from '../../../ui/dom.js';

/** @typedef {import('../model.js').FleetRow} Row */
/** @typedef {import('./layout.js').GraphNode} Node */
export const COLUMN_WIDTH = 124;
const ROW_HEIGHT = 144;

/** @param {Row[]} rows */
export function forestComponents(rows) {
  /** @type {Map<string, Row[]>} */ const membership = new Map();
  /** @type {Row[][]} */ const groups = [];
  for (const row of rows) {
    const parent = row.agent.parent ? membership.get(row.agent.parent) : undefined;
    const group = row.depth > 0 && parent ? parent : [];
    if (!group.length) groups.push(group);
    group.push(row); membership.set(row.agent.id, group);
  }
  invariant(membership.size === rows.length, 'Forest rows must have unique IDs');
  invariant(groups.reduce((count, group) => count + group.length, 0) === rows.length, 'Every row must belong to one component');
  return groups;
}

/** @param {Row[]} rows @param {number} columns */
function componentLayout(rows, columns) {
  invariant(rows.length > 0, 'Components must contain nodes');
  /** @type {Map<number, Row[]>} */ const layers = new Map();
  for (const row of rows) {
    const layer = layers.get(row.depth) || [];
    layer.push(row); layers.set(row.depth, layer);
  }
  const span = Math.min(columns, Math.max(...[...layers.values()].map(layer => layer.length)));
  /** @type {Node[]} */ const nodes = [];
  let height = 0;
  for (const layer of layers.values()) {
    layer.forEach((row, index) => {
      const count = Math.min(span, layer.length - Math.floor(index / span) * span);
      nodes.push({ ...row, x: span * COLUMN_WIDTH / 2 + (index % span - (count - 1) / 2) * COLUMN_WIDTH,
        y: height + 38 + Math.floor(index / span) * ROW_HEIGHT, edgeLane: 6 });
    });
    height += Math.ceil(layer.length / span) * ROW_HEIGHT;
  }
  invariant(nodes.length === rows.length && height > 0, 'Component geometry must cover its rows');
  return { nodes, span, height };
}

/** @param {Row[]} rows @param {number} columns */
export function packForest(rows, columns) {
  invariant(Number.isSafeInteger(columns) && columns > 0, 'Forest columns must be a positive integer');
  const trees = forestComponents(rows).map(group => componentLayout(group, columns));
  const usedColumns = Math.min(columns, Math.max(1, trees.reduce((sum, tree) => sum + tree.span, 0)));
  const heights = Array(usedColumns).fill(0);
  /** @type {Map<string, Node>} */ const positions = new Map();
  const groups = [];
  let previousTop = 0; let previousRight = 0;
  for (const tree of trees) {
    let left = 0; let top = Infinity;
    for (let start = 0; start <= usedColumns - tree.span; start++) {
      // A later component must start after the previous one in top-to-bottom, left-to-right order.
      const height = Math.max(previousTop + (start < previousRight ? ROW_HEIGHT : 0), ...heights.slice(start, start + tree.span));
      if (height < top) { left = start; top = height; }
    }
    invariant(Number.isFinite(top), 'A component must fit in the forest columns');
    const x = left * COLUMN_WIDTH;
    for (const node of tree.nodes) positions.set(node.agent.id, { ...node, x: node.x + x, y: node.y + top, edgeLane: x + 6 });
    groups.push({ id: tree.nodes[0].agent.id, x: x + 1, y: top + 1, width: tree.span * COLUMN_WIDTH - 2, height: tree.height - 12 });
    heights.fill(top + tree.height, left, left + tree.span);
    previousTop = top; previousRight = left + tree.span;
  }
  const nodes = rows.map(row => positions.get(row.agent.id));
  invariant(nodes.every(node => node && Number.isFinite(node.x) && Number.isFinite(node.y)), 'Every forest node needs finite coordinates');
  return { nodes: /** @type {Node[]} */ (nodes), groups, width: usedColumns * COLUMN_WIDTH, height: Math.max(ROW_HEIGHT, ...heights) };
}
