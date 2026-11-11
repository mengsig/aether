import { invariant } from '../../../ui/dom.js';
import { agentState, shortName } from './layout.js';

/** @template {keyof SVGElementTagNameMap} K @param {K} tag @param {Record<string,string|number>} [attributes] */
export function svg(tag, attributes = {}) {
  const element = document.createElementNS('http://www.w3.org/2000/svg', tag);
  for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, String(value));
  return element;
}

/** @param {import('./layout.js').GraphNode} node @param {number} attention @param {(id:string)=>void} open */
export function createNode(node, attention, open) {
  invariant(node.agent.id.length > 0, 'Graph nodes require an agent ID');
  invariant(Number.isSafeInteger(attention) && attention >= 0, 'Attention must be a nonnegative count');
  const group = svg('g', { class: 'fleet-graph-node', role: 'button', tabindex: '0' });
  group.dataset.agentId = node.agent.id;
  group.append(svg('circle', { class: 'fleet-graph-hit', r: 33 }),
    svg('circle', { class: 'fleet-graph-halo', r: 27 }),
    svg('circle', { class: 'fleet-graph-ring', r: 23 }));
  const glyph = svg('text', { class: 'fleet-graph-glyph', y: 6, 'text-anchor': 'middle', 'aria-hidden': 'true' });
  const name = svg('text', { class: 'fleet-graph-name', y: 40, 'text-anchor': 'middle', 'aria-hidden': 'true' });
  name.append(svg('tspan', { x: 0 }), svg('tspan', { x: 0, dy: 17 }));
  const status = svg('text', { class: 'fleet-graph-status', y: 74, 'text-anchor': 'middle', 'aria-hidden': 'true' });
  const badge = svg('g', { class: 'fleet-graph-attention', 'aria-hidden': 'true' });
  badge.append(svg('circle', { cx: 21, cy: -20, r: 10 }), svg('text', { x: 21, y: -16, 'text-anchor': 'middle' }));
  group.append(glyph, name, status, badge);
  group.addEventListener('click', () => open(node.agent.id));
  group.addEventListener('keydown', event => {
    if (event.key !== 'Enter' && event.key !== ' ') return;
    event.preventDefault(); open(node.agent.id);
  });
  updateNode(group, node, attention);
  return group;
}

/** @param {SVGGElement} group @param {import('./layout.js').GraphNode} node @param {number} attention */
export function updateNode(group, node, attention) {
  invariant(group.dataset.agentId === node.agent.id, 'A graph node must retain its identity');
  const name = group.querySelector('.fleet-graph-name');
  invariant(name?.children.length === 2, 'Graph names require two text lines');
  const agent = node.agent;
  const state = agentState(agent);
  const label = agent.name || agent.id;
  group.dataset.state = state;
  group.style.transform = `translate(${node.x}px, ${node.y}px)`;
  group.setAttribute('aria-label', `${label}, ${agent.paused ? 'Paused' : agent.status}${attention ? `, ${attention} awaiting attention` : ''}. Show agent details`);
  const parts = shortName(label);
  updateName(name.children[0], parts[0]); updateName(name.children[1], parts[1]);
  const glyph = group.querySelector('.fleet-graph-glyph');
  if (glyph) glyph.textContent = /claude|anthropic/i.test(agent.model) ? 'A' : /gpt|codex|openai/i.test(agent.model) ? '✳' : '◇';
  const status = group.querySelector('.fleet-graph-status');
  if (status) status.textContent = state === 'failed' ? '× Failed' : state === 'done' ? '✓ Done' : agent.paused ? 'Paused' : agent.status;
  const badge = group.querySelector('.fleet-graph-attention');
  if (badge) badge.setAttribute('visibility', attention ? 'visible' : 'hidden');
  const count = badge?.querySelector('text');
  if (count) count.textContent = attention > 9 ? '9+' : String(attention);
}

/** @param {Element} line @param {string} text */
function updateName(line, text) {
  invariant(line instanceof SVGTSpanElement, 'Node names require SVG text spans');
  invariant([...text].length <= 15, 'Node name lines must be bounded before measuring');
  if (line.dataset.label === text) return;
  line.textContent = text;
  if (!line.isConnected) return;
  const letters = [...text];
  while (line.getComputedTextLength() > 100 && letters.length > 1) {
    letters.pop(); line.textContent = letters.join('') + '…';
  }
  line.dataset.label = text;
}
