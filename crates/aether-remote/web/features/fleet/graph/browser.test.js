import test from 'node:test';
import assert from 'node:assert/strict';
import path from 'node:path';
import { mkdir } from 'node:fs/promises';
import { createMockServer } from '../../../dev/server.js';
import { agent as normalizeAgent } from '../../../core/normalize.js';

const root = path.resolve(import.meta.dirname, '../../..');
const temporary = path.join(root, 'node_modules/fleet-browser-tmp');
await mkdir(temporary, { recursive: true });
process.env.TMPDIR = temporary;
process.env.PLAYWRIGHT_BROWSERS_PATH = path.join(root, 'node_modules/.browsers');
const { chromium } = await import('playwright');
const server = createMockServer();
const skip = process.env.RUN_CONTROLS_BROWSER === '0';
let browser;
let origin;

test.before(async () => {
  if (skip) return;
  await new Promise(resolve => server.http.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${server.http.address().port}`;
  browser = await chromium.launch({ headless: true });
});
test.after(async () => { if (skip) return; if (browser) await browser.close(); await server.close(); });

async function pairedPage(t, secret = 'aether-demo') {
  const context = await browser.newContext({ viewport: { width: 390, height: 844 }, hasTouch: true });
  t.after(() => context.close());
  const page = await context.newPage();
  const errors = []; page.on('pageerror', error => errors.push(error.message));
  t.after(() => assert.deepEqual(errors, []));
  await page.goto(`${origin}/#pair=${secret}`);
  await page.getByRole('button', { name: 'Pair this device' }).click();
  await page.locator('.fleet-graph-node').first().waitFor();
  await page.evaluate(() => { document.documentElement.dataset.motion = 'reduced'; });
  return page;
}

const nodeSelector = '.fleet-graph-node[data-agent-id="agt_00000000000000000000000002"]';
const transform = page => page.locator('.fleet-graph-canvas > g').getAttribute('transform');

test('graph provides zoom, fit, keyboard pan, drag and touch pinch with 44px controls', { skip, timeout: 90000 }, async t => {
  const page = await pairedPage(t);
  const before = await transform(page);
  await page.getByRole('button', { name: 'Zoom in', exact: true }).click();
  assert.notEqual(await transform(page), before);
  await page.getByRole('button', { name: 'Fit graph to screen' }).click();
  assert.ok(Number((await transform(page)).match(/scale\(([^)]+)/)[1]) <= Number(before.match(/scale\(([^)]+)/)[1]));
  await page.locator('.fleet-graph-canvas').focus(); await page.keyboard.press('ArrowLeft');
  const panned = await transform(page); assert.notEqual(panned, before);
  const canvas = await page.locator('.fleet-graph-canvas').boundingBox();
  await page.mouse.move(canvas.x + 15, canvas.y + 15); await page.mouse.down();
  await page.mouse.move(canvas.x + 65, canvas.y + 50, { steps: 5 }); await page.mouse.up();
  assert.notEqual(await transform(page), panned);
  const dragged = await transform(page);
  const cdp = await page.context().newCDPSession(page);
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: canvas.x + 100, y: canvas.y + 200, id: 1 }, { x: canvas.x + 200, y: canvas.y + 200, id: 2 }] });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchMove', touchPoints: [{ x: canvas.x + 70, y: canvas.y + 200, id: 1 }, { x: canvas.x + 230, y: canvas.y + 200, id: 2 }] });
  await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  assert.notEqual(await transform(page), dragged);
  assert.equal(await page.getByRole('dialog').count(), 0);
  for (const control of await page.locator('.fleet-graph-controls button').all()) {
    const box = await control.boundingBox(); assert.ok(box.width >= 44 && box.height >= 44);
  }
});

test('node keyboard and touch open a modal sheet with safe actions and focus return', { skip, timeout: 90000 }, async t => {
  const page = await pairedPage(t); const node = page.locator(nodeSelector);
  await node.focus(); await page.keyboard.press('Enter');
  await page.getByRole('dialog', { name: 'Remote dashboard' }).waitFor();
  assert.equal(await page.getByRole('button', { name: 'Approve…' }).count(), 1);
  await page.keyboard.press('Escape');
  assert.equal(await node.evaluate(element => document.activeElement === element), true);
  await node.tap(); await page.getByRole('button', { name: 'Interrupt', exact: true }).click();
  await page.getByRole('dialog', { name: 'Interrupt this turn?' }).waitFor();
  await page.getByRole('button', { name: 'Keep as is' }).click();
  assert.equal(await page.getByRole('button', { name: 'Interrupt', exact: true }).isEnabled(), true);
  await page.getByRole('button', { name: 'Approve…' }).click(); assert.match(page.url(), /#attention\?item=/);
  await page.goto(`${origin}/#fleet`); await node.waitFor(); await node.click();
  await page.getByRole('button', { name: 'Open chat', exact: true }).click(); assert.match(page.url(), /#agent\/agt_/);
});

test('observers inspect details without interrupt and missing metrics remain unknown', { skip, timeout: 90000 }, async t => {
  const page = await pairedPage(t, 'aether-observer');
  assert.equal(await page.getByRole('button', { name: 'Start work', exact: true }).count(), 0);
  await page.locator(nodeSelector).click();
  assert.equal(await page.getByRole('button', { name: 'Interrupt', exact: true }).count(), 0);
  assert.equal(await page.getByRole('button', { name: 'View approval', exact: true }).count(), 1);
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await page.waitForFunction(selector => document.activeElement?.matches(selector), nodeSelector);
  const nextSprint = page.locator('.fleet-graph-node[data-agent-id="agt_00000000000000000000000007"]');
  await nextSprint.focus(); await nextSprint.click();
  assert.equal(await page.getByRole('dialog').getByText('Not reported', { exact: true }).count(), 2);
});

test('both reduced-motion controls disable animation and large text reflows without page overflow', { skip, timeout: 90000 }, async t => {
  const page = await pairedPage(t);
  const halo = page.locator('.fleet-graph-node[data-state="running"] .fleet-graph-halo').first();
  assert.equal(await halo.evaluate(element => getComputedStyle(element).animationName), 'none');
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.evaluate(() => { delete document.documentElement.dataset.motion; document.documentElement.style.fontSize = '200%'; });
  assert.equal(await halo.evaluate(element => getComputedStyle(element).animationName), 'none');
  for (const width of [320, 390, 1280]) {
    await page.setViewportSize({ width, height: 844 });
    for (const layout of ['List', 'Tree']) {
      await page.getByRole('button', { name: layout, exact: true }).click();
      assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    }
  }
});

test('live graph retains node identity across completion and reparenting and renders unsafe names as text', { skip, timeout: 90000 }, async t => {
  const page = await pairedPage(t);
  const result = await page.evaluate(async agents => {
    const { createGraph } = await import('/features/fleet/graph/canvas.js');
    const graph = createGraph(() => {}); document.querySelector('.fleet-graph').replaceWith(graph.root);
    const state = { agents, attention: [], connection: 'online' }; graph.update(state, 0);
    const original = graph.root.querySelector('[data-agent-id="agt_00000000000000000000000004"]');
    const previousEdge = graph.root.querySelector('[data-child="agt_00000000000000000000000004"]');
    agents[3] = { ...agents[3], status: 'completed', parent: null, name: '<img src=x onerror=alert(1)>' };
    graph.update(state, 0);
    const updated = graph.root.querySelector('[data-agent-id="agt_00000000000000000000000004"]');
    const output = { same: original === updated, state: updated.dataset.state, detached: Boolean(previousEdge) && !previousEdge.isConnected, images: graph.root.querySelectorAll('img').length, label: updated.getAttribute('aria-label') };
    graph.dispose(); return output;
  }, server.state.agents.map(normalizeAgent));
  assert.equal(result.same, true); assert.equal(result.state, 'done'); assert.equal(result.detached, true);
  assert.equal(result.images, 0); assert.match(result.label, /<img/);
});

test('loading has no fabricated counts and large fleet pages bound both layouts', { skip, timeout: 90000 }, async t => {
  const page = await pairedPage(t);
  const result = await page.evaluate(async example => {
    const { mountFleet } = await import('/features/fleet/fleet.js');
    let state = { ready: false, agents: [], attention: [], activity: [], connection: 'connecting', role: 'operator', error: null };
    let update = () => {};
    const host = document.createElement('section'); document.querySelector('main').replaceChildren(host);
    const context = { store: { get: () => state, subscribe: listener => { update = listener; return () => {}; } }, navigate: () => {}, confirm: async () => false, toast: () => {} };
    const dispose = mountFleet(host, context);
    const skeleton = host.querySelectorAll('.fleet-graph-skeleton').length;
    const loadingCounts = host.querySelectorAll('.fleet-summary').length;
    state = { ...state, ready: true, connection: 'online' }; update();
    const quiet = host.textContent.includes('A quiet fleet');
    state = { ...state, agents: Array.from({ length: 500 }, (_, index) => ({ ...example, id: `agent-${index}`, parent: null })) }; update();
    const firstCount = host.querySelectorAll('.fleet-graph-node').length;
    [...host.querySelectorAll('.fleet-pagination button')].find(button => button.textContent === 'Next').click();
    const pageLabel = host.querySelector('.fleet-pagination p').textContent;
    const nextCount = host.querySelectorAll('.fleet-graph-node').length;
    [...host.querySelectorAll('.fleet-switcher button')].find(button => button.textContent === 'List').click();
    const listCount = host.querySelectorAll('.fleet-compact-row').length;
    dispose(); return { skeleton, loadingCounts, quiet, firstCount, nextCount, pageLabel, listCount };
  }, normalizeAgent(server.state.agents[0]));
  assert.deepEqual(result, { skeleton: 1, loadingCounts: 0, quiet: true, firstCount: 80, nextCount: 80, pageLabel: '81–160 of 500 agents', listCount: 80 });
});
