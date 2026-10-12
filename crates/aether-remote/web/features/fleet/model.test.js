import test from 'node:test';
import assert from 'node:assert/strict';
import { approvalCommand, approvalDetails, approvalHeadline, elapsed, fleetRows, questionCommand, usageSummary } from './model.js';

/** @param {Partial<import('../../types.js').Agent>} [patch] @returns {import('../../types.js').Agent} */
function agent(patch = {}) {
  return { id: 'a', name: 'Agent', parent: null, role: 'worker', status: 'running', paused: false,
    model: 'provider/model', effort: 'high', workspace: '', createdAt: '2026-10-12T00:00:00Z',
    updatedAt: '', run: 'run', tokens: null, cost: null, costComplete: false, approval: '', raw: {}, ...patch };
}

/** @param {Partial<import('../../types.js').Attention>} [patch] @returns {import('../../types.js').Attention} */
function attention(patch = {}) {
  return { id: 'approval-1', kind: 'approval', agentId: 'a', title: 'Tool request', detail: 'A request', at: '',
    raw: { tool: 'exec', argument: '  echo "<script>"\n\t$(original)  ', resources: ['file:/a b'], side_effect: 'external' }, ...patch };
}

test('approval headlines describe actions without promoting raw protocol descriptions',()=>{
  const request=attention({detail:'web_fetch {"url":"https://example.com"}',raw:{tool:'web_fetch',argument:'{"url":"https://example.com"}'}});
  assert.equal(approvalHeadline(request),'Fetch a web page');
  assert.equal(approvalHeadline(attention()),'Run a command');
  assert.equal(approvalHeadline(attention({raw:{tool:'unexpected {"command":"rm"}'}})),'Run a tool');
  assert.equal(approvalDetails(request).argument,'{"url":"https://example.com"}');
});

test('tree is depth first and preserves sibling order', () => {
  const rows = fleetRows([agent({ id: 'child', parent: 'root' }), agent({ id: 'root' }),
    agent({ id: 'sibling', parent: 'root' }), agent({ id: 'grandchild', parent: 'child' })]);
  assert.deepEqual(rows.map(row => row.agent.id), ['root', 'child', 'grandchild', 'sibling']);
  assert.deepEqual(rows.map(row => row.depth), [0, 1, 2, 1]);
  assert.ok(rows.every(row => !row.cycle && !row.parentMissing));
});

test('missing parents and cycles do not hide agents or loop', () => {
  const rows = fleetRows([agent({ id: 'orphan', parent: 'absent' }), agent({ id: 'a', parent: 'b' }),
    agent({ id: 'b', parent: 'a' }), agent({ id: 'self', parent: 'self' })]);
  assert.equal(rows.length, 4);
  assert.equal(new Set(rows.map(row => row.agent.id)).size, 4);
  assert.equal(rows[0].parentMissing, true);
  assert.ok(rows.slice(1).every(row => row.cycle));
});

test('duplicate or empty agent IDs fail visibly', () => {
  assert.throws(() => fleetRows([agent(), agent()]), /unique/);
  assert.throws(() => fleetRows([agent({ id: '' })]), /requires an ID/);
});

test('deep and wide fleets stay iterative', () => {
  const deep = Array.from({ length: 10000 }, (_, index) => agent({ id: String(index), parent: index ? String(index - 1) : null }));
  assert.equal(fleetRows(deep).at(-1)?.depth, 9999);
  const wide = deep.map((item, index) => ({ ...item, parent: index ? '0' : null }));
  assert.equal(fleetRows(wide).at(-1)?.depth, 1);
});

test('unknown usage stays unknown and zero stays reported', () => {
  const unknown = usageSummary([agent()]);
  assert.equal(unknown.tokens, null);
  assert.equal(unknown.cost, null);
  assert.equal(unknown.costComplete, false);
  const zero = usageSummary([agent({ tokens: 0, cost: 0, costComplete: true })]);
  assert.equal(zero.tokens, 0);
  assert.equal(zero.cost, 0);
  assert.equal(zero.costComplete, true);
});

test('partial usage totals retain coverage and ignore invalid values', () => {
  const usage = usageSummary([agent({ tokens: 30, cost: 0.25, costComplete: true }),
    agent({ id: 'b', tokens: 10 }), agent({ id: 'c', tokens: NaN, cost: -1 })]);
  assert.equal(usage.tokens, 40);
  assert.equal(usage.cost, 0.25);
  assert.equal(usage.tokenCoverage, 2);
  assert.equal(usage.costCoverage, 1);
  assert.equal(usage.costComplete, false);
});

test('usage totals cannot silently overflow', () => {
  assert.throws(() => usageSummary([agent({ tokens: Number.MAX_VALUE }), agent({ tokens: Number.MAX_VALUE })]), /finite/);
  assert.equal(usageSummary([]).costComplete, false);
});

test('elapsed age handles absent or future timestamps honestly', () => {
  const now = Date.parse('2026-10-12T02:31:00Z');
  assert.equal(elapsed('2026-10-12T00:00:00Z', now), '2h 31m');
  assert.equal(elapsed('', now), 'Not reported');
  assert.equal(elapsed('2026-10-13T00:00:00Z', now), 'Not reported');
});

test('approval arguments are preserved exactly as provided', () => {
  const item = attention();
  const detail = approvalDetails(item);
  assert.equal(detail.argument, item.raw.argument);
  assert.equal(detail.resources, JSON.stringify(item.raw.resources, null, 2));
  assert.equal(detail.effect, 'external');
  assert.deepEqual(JSON.parse(detail.original), item.raw);
});

test('missing approval details are explicitly unknown', () => {
  const details = approvalDetails(attention({ raw: {} }));
  assert.equal(details.argument, 'Not reported by daemon');
  assert.equal(details.resources, 'Not reported by daemon');
  assert.equal(details.effect, 'Not reported by daemon');
});

test('approval decisions retain exact command and target', () => {
  assert.deepEqual(approvalCommand(attention(), 'approved'), { cmd: 'resolve_approval', id: 'approval-1', decision: 'approved' });
  assert.deepEqual(approvalCommand(attention(), 'denied'), { cmd: 'resolve_approval', id: 'approval-1', decision: 'denied' });
  assert.throws(() => approvalCommand(attention({ kind: 'question' }), 'approved'), /Approval requires/);
});

test('question replies are conditional and retain exact content', () => {
  const question = attention({ id: 'question-42', kind: 'question', questionSeq: 42 });
  assert.deepEqual(questionCommand(question, '  Keep this\nformat  '),
    { cmd: 'reply_to_question', to: 'a', question_seq: 42, content: '  Keep this\nformat  ' });
  assert.throws(() => questionCommand(question, '  '), /Write a reply/);
  assert.throws(() => questionCommand({ ...question, questionSeq: undefined }, 'text'), /sequence/);
  assert.throws(() => questionCommand({ ...question, questionSeq: Number.MAX_SAFE_INTEGER + 1 }, 'text'), /sequence/);
});
