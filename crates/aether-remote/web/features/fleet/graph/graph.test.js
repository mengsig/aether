import test from 'node:test';
import assert from 'node:assert/strict';
import { agentState, edgePath, graphLayout, PAGE_SIZE, shortName } from './layout.js';
import { interruptAgent } from './sheet.js';

const agent = (id, parent = null, status = 'running') => ({ id, parent, status, name: id, paused: false });

test('graph preserves each cycle and orphan once without recursive traversal', () => {
  const agents = [agent('root'), agent('child', 'root'), agent('orphan', 'missing'), agent('a', 'b'), agent('b', 'a'), agent('self', 'self')];
  const layout = graphLayout(agents);
  assert.equal(layout.nodes.length, agents.length);
  assert.equal(new Set(layout.nodes.map(node => node.agent.id)).size, agents.length);
  assert.equal(layout.nodes.find(node => node.agent.id === 'orphan').parentMissing, true);
  assert.equal(layout.nodes.find(node => node.agent.id === 'a').cycle, true);
  assert.equal(layout.nodes.find(node => node.agent.id === 'self').cycle, true);
  assert.ok(layout.nodes.every(node => Number.isFinite(node.x) && Number.isFinite(node.y)));
});

test('large and deep fleets have bounded pages and complete traversal', () => {
  const agents = Array.from({ length: 10000 }, (_, index) => agent(`agent-${index}`, index ? `agent-${index - 1}` : null));
  const first = graphLayout(agents);
  const last = graphLayout(agents, Math.ceil(agents.length / PAGE_SIZE) - 1);
  assert.equal(first.nodes.length, PAGE_SIZE);
  assert.equal(last.nodes.at(-1).agent.id, 'agent-9999');
  assert.equal(last.total, 10000);
  assert.ok(first.height < 150 * PAGE_SIZE);
  assert.throws(() => graphLayout(agents, -1), /nonnegative/);
  assert.throws(() => graphLayout(agents, 10000), /range/);
  assert.throws(() => graphLayout([agent('same'), agent('same')]), /unique/);
});

test('reparented nodes move and edges remain finite', () => {
  const agents = [agent('root'), agent('child', 'root'), agent('leaf', 'child')];
  const before = graphLayout(agents);
  const after = graphLayout([agents[0], agents[1], { ...agents[2], parent: 'root' }]);
  const leafBefore = before.nodes.find(node => node.agent.id === 'leaf');
  const leafAfter = after.nodes.find(node => node.agent.id === 'leaf');
  assert.ok(leafBefore.y > leafAfter.y);
  assert.match(edgePath(after.nodes[0], leafAfter), /^M .+ H /);
  assert.throws(() => edgePath(leafAfter, leafAfter), /Self/);
  assert.throws(() => edgePath({ ...after.nodes[0], x: NaN }, leafAfter), /finite/);
});

test('state indicators distinguish failure, pause and completion; names are bounded', () => {
  assert.equal(agentState(agent('a', null, 'failed')), 'failed');
  assert.equal(agentState({ ...agent('a'), paused: true }), 'waiting');
  assert.equal(agentState(agent('a', null, 'completed')), 'done');
  assert.equal(agentState(agent('a', null, 'ready')), 'ready');
  assert.ok(shortName('a'.repeat(100) + ' ' + 'b'.repeat(100)).every(part => [...part].length <= 15));
});

function mutationContext() {
  const state = { role: 'operator', connection: 'online', agents: [agent('selected')] };
  const commands = [];
  const context = { store: { get: () => state }, confirm: async () => true, command: async command => { commands.push(command); }, toast: () => {} };
  return { state, commands, context };
}

test('interrupt sends the exact command only after confirmation', async () => {
  const { context, commands } = mutationContext();
  await interruptAgent(context, 'selected', () => true);
  assert.deepEqual(commands, [{ cmd: 'interrupt', agent: 'selected' }]);
  context.confirm = async () => false;
  await interruptAgent(context, 'selected', () => true);
  assert.equal(commands.length, 1);
});

test('interrupt rechecks role, connection, existence, running status and view lifetime after confirmation', async () => {
  for (const mutate of [state => { state.role = 'observer'; }, state => { state.connection = 'offline'; }, state => { state.agents = []; }, state => { state.agents[0].status = 'completed'; }]) {
    const { state, context, commands } = mutationContext();
    context.confirm = async () => { mutate(state); return true; };
    await assert.rejects(interruptAgent(context, 'selected', () => true));
    assert.deepEqual(commands, []);
  }
  const { context, commands } = mutationContext();
  await assert.rejects(interruptAgent(context, 'selected', () => false), /closed/);
  assert.deepEqual(commands, []);
});

test('independent roots pack into phone columns while actual descendants stay below their parents', () => {
  const agents = [...Array.from({ length: 8 }, (_, index) => agent(`root-${index}`)), agent('child', 'root-0'), agent('leaf', 'child')];
  for (const width of [270, 340, 135, 170, 1200]) {
    const layout = graphLayout(agents, 0, width);
    const byId = new Map(layout.nodes.map(node => [node.agent.id, node]));
    assert.equal(layout.nodes.length, 10); assert.ok(layout.width <= Math.max(124, width));
    assert.ok(byId.get('child').y > byId.get('root-0').y); assert.ok(byId.get('leaf').y > byId.get('child').y);
    if (width < 400) assert.ok(new Set(layout.nodes.map(node => node.y)).size >= 4);
    for (const node of layout.nodes) for (const other of layout.nodes) {
      if (node === other) continue;
      assert.ok(Math.abs(node.x - other.x) >= 112 || Math.abs(node.y - other.y) >= 120);
    }
  }
  assert.throws(() => graphLayout(agents, 0, NaN), /positive/);
  assert.throws(() => graphLayout(agents, 0, 0), /positive/);
});

test('wide sibling groups wrap without losing parent edges or page membership', () => {
  const agents = [agent('root'), ...Array.from({ length: 99 }, (_, index) => agent(`child-${index}`, 'root'))];
  const first = graphLayout(agents, 0, 270); const second = graphLayout(agents, 1, 270);
  assert.equal(first.nodes.length, 80); assert.equal(second.nodes.length, 20);
  assert.equal(new Set([...first.nodes, ...second.nodes].map(node => node.agent.id)).size, 100);
  assert.ok(first.nodes.slice(1).every(node => node.y > first.nodes[0].y && node.agent.parent === 'root'));
  assert.ok(first.width <= 270 && second.width <= 270);
});

test('whole trees rank by descendant attention, running, waiting, ready, then done before paging', () => {
  const agents = [agent('done', null, 'completed'), agent('waiting', null, 'waiting'), agent('ready', null, 'ready'),
    agent('active'), agent('active-child', 'active', 'completed'), agent('needs', null, 'completed'), agent('approval', 'needs', 'waiting'), agent('failed', null, 'failed')];
  const attention = new Map([['approval', 1]]);
  const layout = graphLayout(agents, 0, 340, attention);
  assert.deepEqual(layout.groups.map(group => group.id), ['needs', 'failed', 'active', 'waiting', 'ready', 'done']);
  assert.deepEqual(layout.nodes.map(node => node.agent.id), ['needs', 'approval', 'failed', 'active', 'active-child', 'waiting', 'ready', 'done']);
  assert.equal(layout.nodes.find(node => node.agent.id === 'approval').agent.parent, 'needs');
  const many = [...Array.from({ length: 90 }, (_, index) => agent(`done-${index}`, null, 'completed')), ...agents.slice(5, 7)];
  assert.equal(graphLayout(many, 0, 340, attention).nodes[0].agent.id, 'needs');
  assert.equal(graphLayout(many, 1, 340, attention).nodes.length, 12);
  assert.throws(() => graphLayout(agents, 0, 340, new Map([['approval', -1]])), /counts/);
});

test('component bounds never overlap, contain their nodes, and retain priority reading order', () => {
  const agents = [agent('first'), ...Array.from({ length: 9 }, (_, index) => agent(`child-${index}`, 'first')),
    agent('second'), agent('third'), agent('fourth'), agent('fifth')];
  for (const width of [170, 340, 500, 1200]) {
    const layout = graphLayout(agents, 0, width);
    for (const [index, group] of layout.groups.entries()) {
      const previous = layout.groups[index - 1];
      if (previous) assert.ok(group.y > previous.y || group.y === previous.y && group.x > previous.x);
      for (const other of layout.groups.slice(index + 1)) {
        assert.ok(group.x + group.width <= other.x || other.x + other.width <= group.x || group.y + group.height <= other.y || other.y + other.height <= group.y);
      }
      const members = layout.nodes.filter(node => node.agent.id === group.id || node.agent.parent === group.id);
      assert.ok(members.length > 0);
      assert.ok(members.every(node => node.x - 50 >= group.x && node.x + 50 <= group.x + group.width && node.y - 33 >= group.y && node.y + 80 <= group.y + group.height));
    }
  }
});

test('wrapped sibling edges use exterior gutters and clear every label row', () => {
  const agents = [agent('root'), ...Array.from({ length: 17 }, (_, index) => agent(`child-${index}`, 'root')), agent('leaf', 'child-3')];
  const layout = graphLayout(agents, 0, 340);
  const byId = new Map(layout.nodes.map(node => [node.agent.id, node]));
  for (const node of layout.nodes.slice(1)) {
    const parent = byId.get(node.agent.parent);
    const numbers = edgePath(parent, node).match(/-?\d+(?:\.\d+)?/g).map(Number);
    const [x, y, side, exit, gutter, entry, target, end] = numbers;
    const points = [[x, y], [side, y], [side, exit], [gutter, exit], [gutter, entry], [target, entry], [target, end]];
    assert.equal(end, node.y - 27); assert.equal(x, parent.x - 27);
    assert.ok(points.reduce((length, point, index) => index ? length + Math.hypot(point[0] - points[index - 1][0], point[1] - points[index - 1][1]) : 0, 0) > 100);
    for (let index = 1; index < points.length; index++) {
      const [a, b] = [points[index - 1], points[index]];
      for (const label of layout.nodes) {
        assert.ok(Math.max(a[0], b[0]) < label.x - 52 || Math.min(a[0], b[0]) > label.x + 52 || Math.max(a[1], b[1]) < label.y + 24 || Math.min(a[1], b[1]) > label.y + 80);
      }
    }
  }
});
