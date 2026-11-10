import { invariant } from '../../../ui/dom.js';
import { fleetRows } from '../model.js';
import { COLUMN_WIDTH, forestComponents, packForest } from './packing.js';

/** @typedef {import('../../../types.js').Agent} Agent */
/** @typedef {{agent:Agent,x:number,y:number,parentMissing:boolean,cycle:boolean,edgeLane:number}} GraphNode */
export const PAGE_SIZE = 80;

/** @param {Agent} agent */
export function agentState(agent) {
  if (['failed', 'error'].includes(agent.status.toLowerCase())) return 'failed';
  if (['completed', 'done', 'cancelled', 'canceled'].includes(agent.status.toLowerCase())) return 'done';
  if (agent.paused || ['waiting', 'blocked', 'paused'].includes(agent.status.toLowerCase())) return 'waiting';
  if (['running', 'working', 'active'].includes(agent.status.toLowerCase())) return 'running';
  return 'ready';
}

/** @param {Agent[]} agents @param {number} page @param {number} width @param {Map<string,number>} attention */
export function graphLayout(agents, page = 0, width = 340, attention = new Map()) {
  invariant(Number.isSafeInteger(page) && page >= 0, 'Graph page must be a nonnegative integer');
  invariant(Number.isFinite(width) && width > 0, 'Graph width must be positive');
  invariant(attention instanceof Map, 'Graph attention must be an agent count map');
  invariant([...attention.values()].every(count => Number.isSafeInteger(count) && count >= 0), 'Graph attention counts must be nonnegative integers');
  const priority = { failed: 0, running: 1, waiting: 2, ready: 3, done: 4 };
  const groups = forestComponents(fleetRows(agents)).map(rows => ({ rows,
    rank: rows.reduce((rank, row) => Math.min(rank, attention.get(row.agent.id) ? 0 : priority[agentState(row.agent)]), 4) }));
  const rows = groups.sort((a, b) => a.rank - b.rank).flatMap(group => group.rows);
  invariant(page === 0 || page * PAGE_SIZE < rows.length, 'Graph page must be in range');
  const visible = rows.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);
  const layout = packForest(visible, Math.max(1, Math.floor(width / COLUMN_WIDTH)));
  invariant(layout.nodes.length <= PAGE_SIZE && new Set(layout.nodes.map(node => node.agent.id)).size === visible.length, 'Graph nodes must be bounded and unique');
  return { ...layout, total: rows.length };
}

/** @param {GraphNode} parent @param {GraphNode} child */
export function edgePath(parent, child) {
  invariant(parent.agent.id !== child.agent.id, 'Self edges are not rendered');
  invariant([parent.x, parent.y, child.x, child.y].every(Number.isFinite), 'Edge coordinates must be finite');
  invariant(parent.y < child.y, 'Edges must point to a later descendant row');
  invariant(Number.isFinite(child.edgeLane) && child.edgeLane <= Math.min(parent.x, child.x) - 56, 'Edge gutter must stay outside node labels');
  // Leave beside the parent, clear its status, then follow the outer gutter past wrapped rows.
  return `M ${parent.x - 27} ${parent.y} H ${parent.x - 56} V ${parent.y + 88} H ${child.edgeLane} V ${child.y - 40} H ${child.x} V ${child.y - 27}`;
}

/** @param {string} name */
export function shortName(name) {
  const words = name.split(/\s+/);
  const first = words.shift() || name;
  const rest = words.join(' ');
  const shorten = (/** @type {string} */ value) => [...value].length > 15 ? [...value].slice(0, 14).join('') + '…' : value;
  return [shorten(first), shorten(rest)];
}
