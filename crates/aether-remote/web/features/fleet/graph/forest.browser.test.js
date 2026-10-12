import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { mkdir } from 'node:fs/promises';
import { createMockServer } from '../../../dev/server.js';
import { agent as normalizeAgent } from '../../../core/normalize.js';

const root = path.resolve(import.meta.dirname, '../../..');
const artifacts = path.join(root, 'node_modules/fleet-forest-artifacts');
await mkdir(artifacts, { recursive: true });
process.env.TMPDIR = artifacts;
process.env.PLAYWRIGHT_BROWSERS_PATH = path.join(root, 'node_modules/.browsers');
const { chromium } = await import('playwright');
const skip = process.env.RUN_CONTROLS_BROWSER === '0';
const server = createMockServer();
let browser;
let origin;
test.before(async () => {
  if (skip) return;
  await new Promise(resolve => server.http.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${server.http.address().port}`;
  browser = await chromium.launch({ headless: true });
});
test.after(async () => { if (skip) return; if (browser) await browser.close(); await server.close(); });

function forest(count) {
  assert.ok(Number.isSafeInteger(count) && count > 0);
  const names = ['Workspace lead', 'Live resolution', 'Live approval denied', 'Live approval approved', 'Mobile acceptance', 'Live question', 'Deployment review', 'Release decision', 'Worker one', 'Worker two'];
  const example = normalizeAgent(server.state.agents[0]);
  const agents = Array.from({ length: count }, (_, index) => ({ ...example, id: `forest-${index}`, name: names[index] || `Worker ${index}`,
    status: index % 3 ? 'waiting' : 'completed', parent: index === 8 ? 'forest-0' : index === 9 ? 'forest-8' : null }));
  assert.equal(agents.filter(agent => agent.parent).length, Math.min(2, Math.max(0, count - 8)));
  return agents;
}

async function forestPage(t, { width = 390, text = 100, theme = 'light', count = 10 } = {}) {
  const context = await browser.newContext({ viewport: { width, height: 844 }, hasTouch: true });
  t.after(() => context.close());
  const page = await context.newPage();
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.goto(`${origin}/#pair=aether-demo`);
  await page.getByRole('button', { name: 'Pair this device' }).click();
  await page.locator('.fleet-graph-node').first().waitFor();
  await page.evaluate(async ({ agents, text, theme }) => {
    document.documentElement.dataset.motion = 'reduced'; document.documentElement.dataset.theme = theme;
    document.documentElement.style.fontSize = `${text}%`;
    const { mountFleet } = await import('/features/fleet/fleet.js');
    const host = document.createElement('section'); document.querySelector('main').replaceChildren(host);
    let state = { ready: true, agents, attention: [], activity: [], connection: 'online', role: 'operator', error: null };
    let update = () => {};
    const context = { store: { get: () => state, subscribe: listener => { update = listener; return () => {}; } }, navigate: () => {}, confirm: async () => false, toast: () => {} };
    window.forestTest = { update: (agents, attention = []) => { state = { ...state, agents, attention }; update(); }, dispose: mountFleet(host, context) };
  }, { agents: forest(count), text, theme });
  await page.locator('.fleet-graph-node').first().waitFor();
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  return page;
}

async function geometry(page) {
  return page.locator('.fleet-graph').evaluate(root => {
    const canvas = root.querySelector('svg'); const area = canvas.getBoundingClientRect();
    const box = element => { const rect = element.getBoundingClientRect(); return { x: rect.x, y: rect.y, width: rect.width, height: rect.height }; };
    const nodes = [...root.querySelectorAll('.fleet-graph-node')].map(node => ({ id: node.dataset.agentId, hit: box(node.querySelector('.fleet-graph-hit')),
      statusSize: parseFloat(getComputedStyle(node.querySelector('.fleet-graph-status')).fontSize) * node.getScreenCTM().a,
      labelSize: parseFloat(getComputedStyle(node.querySelector('.fleet-graph-name')).fontSize) * node.getScreenCTM().a }));
    return { nodes, canvas: box(canvas), visible: nodes.filter(node => node.hit.x >= area.left && node.hit.x + node.hit.width <= area.right && node.hit.y >= area.top && node.hit.y + node.hit.height <= area.bottom).length,
      edges: [...root.querySelectorAll('.fleet-graph-edge')].map(edge => edge.dataset.child), scale: canvas.querySelector('g').getCTM().a,
      overflow: document.documentElement.scrollWidth > innerWidth, controls: root.querySelectorAll('.fleet-graph-controls button').length };
  });
}

for (const width of [390, 360]) for (const text of [100, 200]) for (const theme of ['light', 'dark']) {
  test(`eight roots and a chain remain readable at ${width}px ${text}% ${theme}`, { skip, timeout: 90000 }, async t => {
    const page = await forestPage(t, { width, text, theme }); const result = await geometry(page);
    assert.equal(result.nodes.length, 10); assert.deepEqual(result.edges, ['forest-8', 'forest-9']);
    assert.ok(result.nodes.every(node => node.hit.width >= 59 && node.hit.height >= 59 && node.labelSize >= 12.5 && node.statusSize >= 12.5));
    assert.equal(result.controls, 3); assert.equal(result.overflow, false);
    assert.ok(new Set(result.nodes.map(node => Math.round(node.hit.y))).size >= 4);
    assert.ok(result.visible >= (text === 200 ? 1 : width === 390 ? 9 : 6));
    await page.screenshot({ path: path.join(artifacts, `forest-10-${width}-${text}-${theme}.png`), fullPage: true });
    await assertNavigationClearance(page);
  });
}

for (const count of [1, 60]) for (const width of [390, 360]) for (const text of [100, 200]) for (const theme of ['light', 'dark']) {
  test(`${count} agents remain readable at ${width}px ${text}% ${theme}`, { skip, timeout: 90000 }, async t => {
    const page = await forestPage(t, { count, width, text, theme }); const before = await geometry(page);
    assert.equal(before.nodes.length, count); assert.ok(before.nodes.every(node => node.labelSize >= 12.5 && node.statusSize >= 12.5 && node.hit.width >= 59));
    assert.equal(before.overflow, false);
    if (count === 1) assert.ok(before.canvas.height <= 220 * text / 100);
    else { assert.ok((before.visible >= (text === 200 ? 1 : width === 390 ? 9 : 6))); assert.match(await page.locator('.fleet-graph-viewport-status').textContent(), /Drag to explore/); }
    await page.screenshot({ path: path.join(artifacts, `forest-${count}-${width}-${text}-${theme}.png`), fullPage: true });
    await assertNavigationClearance(page);
    await page.getByRole('button', { name: 'Fit graph to screen' }).click();
    const fitted = await geometry(page); assert.equal(fitted.visible, count);
    if (count === 60) assert.ok(fitted.scale < before.scale);
  });
}

test('manual camera and node identity survive live updates and responsive repacking; keyboard reaches the last of 80 nodes', { skip, timeout: 90000 }, async t => {
  const page = await forestPage(t, { count: 80 });
  await page.locator('.fleet-graph-canvas').focus(); await page.keyboard.press('ArrowDown');
  const before = await page.locator('.fleet-graph-canvas > g').getAttribute('transform');
  const retained = await page.evaluate(agents => {
    const node = document.querySelector('[data-agent-id="forest-0"]');
    window.forestTest.update(agents);
    return node === document.querySelector('[data-agent-id="forest-0"]');
  }, forest(80).map((agent, index) => ({ ...agent, status: 'running', parent: index === 9 ? 'forest-0' : agent.parent })));
  assert.equal(retained, true); assert.equal(await page.locator('.fleet-graph-canvas > g').getAttribute('transform'), before);
  await page.setViewportSize({ width: 320, height: 844 });
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(await page.locator('.fleet-graph-canvas > g').getAttribute('transform'), before);
  const last = page.locator('[data-agent-id="forest-79"]'); await last.focus();
  const result = await geometry(page); assert.equal(result.nodes.length, 80); assert.ok(result.nodes.at(-1).hit.y >= result.canvas.y);
  assert.ok(result.nodes.at(-1).hit.y + result.nodes.at(-1).hit.height <= result.canvas.y + result.canvas.height);
  await page.keyboard.press('Enter'); await page.getByRole('dialog', { name: 'Worker 79' }).waitFor();
});

test('graph controls stay above phone navigation and wide desktops gain columns', { skip, timeout: 90000 }, async t => {
  const page = await forestPage(t);
  for (const viewport of [{ width: 390, height: 844 }, { width: 360, height: 640 }, { width: 1280, height: 844 }]) {
    await page.setViewportSize(viewport);
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const result = await geometry(page);
    assert.equal(result.overflow, false); assert.equal(result.nodes.length, 10);
    const columns = new Set(result.nodes.map(node => Math.round(node.hit.x))).size;
    assert.ok(columns >= (viewport.width > 1000 ? 5 : 2));
    for (const control of await page.locator('.fleet-graph-controls button').all()) {
      assert.ok(await control.evaluate(element => {
        const box = element.getBoundingClientRect();
        return element.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
      }));
    }
    await page.screenshot({ path: path.join(artifacts, `forest-10-${viewport.width}x${viewport.height}-controls.png`), fullPage: true });
  }
});

test('wide labels stay inside their columns while accessible names remain complete', { skip, timeout: 90000 }, async t => {
  const page = await forestPage(t);
  const agents = forest(10); agents[0].name = 'W'.repeat(40) + ' ' + 'W'.repeat(40);
  await page.evaluate(agents => window.forestTest.update(agents), agents);
  const node = page.locator('[data-agent-id="forest-0"]');
  assert.match(await node.getAttribute('aria-label'), new RegExp('W'.repeat(40)));
  const widths = await node.locator('tspan').evaluateAll(lines => lines.map(line => line.getComputedTextLength()));
  assert.ok(widths.every(width => width <= 100));
  assert.ok((await node.locator('.fleet-graph-name').textContent()).includes('…'));
});

async function hierarchyGeometry(page) {
  return page.locator('.fleet-graph').evaluate(root => {
    const box = element => { const r = element.getBoundingClientRect(); return { left: r.left, top: r.top, right: r.right, bottom: r.bottom }; };
    const labels = [...root.querySelectorAll('.fleet-graph-name, .fleet-graph-status')].map(box);
    const groups = [...root.querySelectorAll('.fleet-graph-group')].map(group => ({ id: group.dataset.root, ...box(group) }));
    const nodes = [...root.querySelectorAll('.fleet-graph-node')].map(node => ({ id: node.dataset.agentId, ...box(node) }));
    const edges = [...root.querySelectorAll('.fleet-graph-edge')].map(edge => {
      const length = edge.getTotalLength(); const matrix = edge.getScreenCTM(); let crossesLabel = false;
      for (let distance = 0; distance <= length; distance += 2) {
        const point = edge.getPointAtLength(distance).matrixTransform(matrix);
        if (labels.some(label => point.x >= label.left - 1 && point.x <= label.right + 1 && point.y >= label.top - 1 && point.y <= label.bottom + 1)) crossesLabel = true;
      }
      return { child: edge.dataset.child, length, crossesLabel, strokeWidth: parseFloat(getComputedStyle(edge).strokeWidth) };
    });
    return { groups, nodes, edges };
  });
}

for (const theme of ['light', 'dark']) test(`wrapped sibling connectors clear rendered labels within tree boundaries in ${theme}`, { skip, timeout: 90000 }, async t => {
  const page = await forestPage(t, { count: 20, theme });
  const agents = forest(20).map((agent, index) => ({ ...agent, status: 'running', parent: index === 0 || index === 19 ? null : index < 15 ? 'forest-0' : 'forest-1' }));
  await page.evaluate(agents => window.forestTest.update(agents), agents);
  const result = await hierarchyGeometry(page);
  assert.equal(result.groups.length, 2); assert.equal(result.edges.length, 18);
  assert.ok(result.edges.every(edge => edge.length > 100 && !edge.crossesLabel && edge.strokeWidth >= 2));
  for (const group of result.groups) {
    const members = result.nodes.filter(node => group.id === 'forest-19' ? node.id === group.id : node.id !== 'forest-19');
    assert.ok(members.every(node => node.left >= group.left && node.right <= group.right && node.top >= group.top && node.bottom <= group.bottom));
  }
  const [a, b] = result.groups;
  assert.ok(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top);
  await page.screenshot({ path: path.join(artifacts, `wrapped-siblings-${theme}.png`), fullPage: true });
});

test('attention promotes the entire tree and preserves node identity and manual camera', { skip, timeout: 90000 }, async t => {
  const page = await forestPage(t);
  const agents = forest(10).map((agent, index) => ({ ...agent, status: index === 1 ? 'running' : index === 2 ? 'waiting' : 'completed' }));
  await page.evaluate(agents => window.forestTest.update(agents), agents);
  const before = await hierarchyGeometry(page);
  assert.deepEqual(before.groups.slice(0, 2).map(group => group.id), ['forest-1', 'forest-2']);
  await page.locator('.fleet-graph-canvas').focus(); await page.keyboard.press('ArrowDown');
  const camera = await page.locator('.fleet-graph-canvas > g').getAttribute('transform');
  const identity = await page.evaluate(agents => {
    const nodes = [...document.querySelectorAll('.fleet-graph-node')];
    window.forestTest.update(agents, [{ id: 'attention', agentId: 'forest-9', kind: 'approval' }]);
    return nodes.every(node => node === document.querySelector(`[data-agent-id="${node.dataset.agentId}"]`));
  }, agents);
  const after = await hierarchyGeometry(page);
  assert.equal(identity, true); assert.equal(await page.locator('.fleet-graph-canvas > g').getAttribute('transform'), camera);
  assert.deepEqual(after.groups.slice(0, 3).map(group => group.id), ['forest-0', 'forest-1', 'forest-2']);
  assert.equal(after.edges.length, 2); assert.ok(after.edges.every(edge => !edge.crossesLabel));
  for (let index = 1; index < after.groups.length; index++) {
    const previous = after.groups[index - 1]; const group = after.groups[index];
    assert.ok(group.top > previous.top || group.top === previous.top && group.left > previous.left);
  }
});

async function assertNavigationClearance(page) {
  const controls = page.locator('.fleet-graph-controls button');
  assert.equal(await controls.count(), 3);
  for (const control of await controls.all()) {
    await control.scrollIntoViewIfNeeded();
    assert.equal(await control.evaluate(element => {
      const box = element.getBoundingClientRect();
      const nav = document.querySelector('[aria-label="Main navigation"]').getBoundingClientRect();
      return box.bottom <= nav.top && element.contains(document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2));
    }), true);
  }
}
