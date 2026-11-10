import { el, invariant } from '../../../ui/dom.js';
import { agentState, edgePath, graphLayout } from './layout.js';
import { svg, createNode, updateNode } from './nodes.js';
import { createViewport } from './viewport.js';

/** @param {(id:string)=>void} open */
export function createGraph(open) {
  invariant(typeof open === 'function', 'Graph selection requires a callback');
  invariant(typeof ResizeObserver === 'function', 'Graph layout requires resize observation');
  const root = el('section', 'fleet-graph');
  root.setAttribute('aria-label', 'Agent delegation graph');
  const canvas = svg('svg', { class: 'fleet-graph-canvas', 'aria-label': 'Agent delegation tree. Drag or use arrow keys to pan; select an agent for details.', role: 'group', tabindex: '0' });
  const scene = svg('g'); const edges = svg('g', { class: 'fleet-graph-edges', 'aria-hidden': 'true' });
  const groups = svg('g', { class: 'fleet-graph-groups', 'aria-hidden': 'true' });
  const nodes = svg('g'); scene.append(groups, edges, nodes); canvas.append(scene);
  const viewport = createViewport(canvas, scene);
  const help = el('p', 'fleet-graph-help', 'Drag or arrows to pan · Pinch to zoom');
  const anomalies = el('p', 'fleet-graph-anomalies muted');
  const label = el('div', 'fleet-graph-caption');
  const liveLabel = el('span', 'eyebrow', 'Live graph');
  label.append(liveLabel, help);
  root.append(label, canvas, viewport.controls, anomalies);
  /** @type {Map<string,SVGGElement>} */ const elements = new Map();
  let geometry = '';
  /** @type {import('../../../types.js').State|null} */ let latest = null;
  let currentPage = 0;
  const update = (/** @type {import('../../../types.js').State} */ state, /** @type {number} */ page) => {
    invariant(Array.isArray(state.attention), 'Graph attention must be an array');
    latest = state; currentPage = page;
    const width = canvas.clientWidth || 340;
    const readableScale = Math.max(1, parseFloat(getComputedStyle(document.documentElement).fontSize) / 16) * .9;
    geometry = `${width}:${readableScale}:${innerHeight}`;
    const attention = new Map();
    for (const item of state.attention) attention.set(item.agentId, (attention.get(item.agentId) || 0) + 1);
    const layout = graphLayout(state.agents, page, width / readableScale, attention);
    sizeCanvas(canvas, viewport.controls, layout.height, readableScale);
    updateGroups(groups, layout.groups);
    const current = new Set(layout.nodes.map(node => node.agent.id));
    updateNodes(nodes, elements, layout.nodes, attention, open);
    updateEdges(edges, layout.nodes);
    const missing = layout.nodes.some(node => node.parentMissing);
    const offPage = layout.nodes.some(node => node.agent.parent && !current.has(node.agent.parent) && !node.parentMissing);
    const cycle = layout.nodes.some(node => node.cycle);
    anomalies.textContent = [missing ? 'Some parents are outside this fleet.' : '', offPage ? 'Some parents are on another page.' : '', cycle ? 'A hierarchy cycle is shown once; use List for parent details.' : ''].filter(Boolean).join(' ');
    anomalies.hidden = !anomalies.textContent;
    root.dataset.live = String(state.connection === 'online');
    liveLabel.textContent = state.connection === 'online' ? 'Live graph' : 'Last known graph';
    viewport.update({ ...layout, readableScale });
  };
  const resize = () => {
    const scale = Math.max(1, parseFloat(getComputedStyle(document.documentElement).fontSize) / 16) * .9;
    if (latest && canvas.clientWidth && geometry !== `${canvas.clientWidth}:${scale}:${innerHeight}`) update(latest, currentPage);
  };
  const observer = new ResizeObserver(resize); observer.observe(root);
  window.addEventListener('resize', resize);
  return { root, update, dispose: () => { observer.disconnect(); window.removeEventListener('resize', resize); viewport.dispose(); } };
}

/** @param {SVGGElement} edges @param {import('./layout.js').GraphNode[]} nodes */
function updateEdges(edges, nodes) {
  const byId = new Map(nodes.map(node => [node.agent.id, node]));
  invariant(byId.size === nodes.length, 'Graph edge endpoints must be unique');
  invariant(edges.classList.contains('fleet-graph-edges'), 'Edges require their own SVG layer');
  const existing = new Map([...edges.children].map(node => [/** @type {SVGElement} */ (node).dataset.child, node]));
  const retained = new Set();
  for (const node of nodes) {
    const parent = node.agent.parent ? byId.get(node.agent.parent) : null;
    if (!parent || parent.y >= node.y) continue;
    const edge = existing.get(node.agent.id) || svg('path', { class: 'fleet-graph-edge', fill: 'none' });
    edge.setAttribute('d', edgePath(parent, node));
    edge.setAttribute('data-child', node.agent.id);
    edge.setAttribute('data-state', agentState(node.agent));
    if (!existing.has(node.agent.id)) edges.append(edge);
    retained.add(node.agent.id);
  }
  for (const [id, edge] of existing) if (!retained.has(id)) edge.remove();
}

/** @param {SVGSVGElement} canvas @param {HTMLElement} controls @param {number} height @param {number} scale */
function sizeCanvas(canvas, controls, height, scale) {
  invariant(Number.isFinite(height) && height > 0, 'Canvas content height must be positive');
  invariant(Number.isFinite(scale) && scale >= .9, 'Initial graph scale must remain readable');
  const nav = document.querySelector('[aria-label="Main navigation"]')?.getBoundingClientRect();
  const bottom = nav && nav.width > innerWidth / 2 ? nav.top - 12 : innerHeight - 24;
  const top = canvas.getBoundingClientRect().top + scrollY;
  const available = Math.max(180 * scale, bottom - top - controls.offsetHeight);
  canvas.style.height = `${Math.min(height * scale + 24, 580, available)}px`;
}

/** @param {SVGGElement} layer @param {ReturnType<typeof graphLayout>['groups']} groups */
function updateGroups(layer, groups) {
  invariant(new Set(groups.map(group => group.id)).size === groups.length, 'Tree boundaries require unique roots');
  invariant(groups.every(group => group.width > 0 && group.height > 0), 'Tree boundaries must have positive dimensions');
  layer.replaceChildren(...groups.map(group => svg('rect', { class: 'fleet-graph-group',
    'data-root': group.id, x: group.x, y: group.y, width: group.width, height: group.height, rx: 14 })));
}

/** @param {SVGGElement} layer @param {Map<string,SVGGElement>} elements @param {import('./layout.js').GraphNode[]} nodes @param {Map<string,number>} attention @param {(id:string)=>void} open */
function updateNodes(layer, elements, nodes, attention, open) {
  const current = new Set(nodes.map(node => node.agent.id));
  invariant(current.size === nodes.length, 'Rendered graph nodes must be unique');
  invariant(layer.children.length === elements.size, 'Node identity cache must match its SVG layer');
  for (const [id, element] of elements) if (!current.has(id)) { element.remove(); elements.delete(id); }
  for (const node of nodes) {
    let element = elements.get(node.agent.id);
    if (!element) {
      element = createNode(node, attention.get(node.agent.id) || 0, open);
      elements.set(node.agent.id, element); layer.append(element);
    }
    updateNode(element, node, attention.get(node.agent.id) || 0);
  }
}
