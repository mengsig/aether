import test from 'node:test';
import assert from 'node:assert/strict';
import { count, dollars, dateLabel, fileSize, recurrenceLabel, scopeLabel, costCoverage } from './format.js';

test('numbers retain zero, distinguish missing values and reject invalid accounting', () => {
  assert.equal(dollars(7.289999999999999), '$7.29');
  assert.equal(dollars(0), '$0.00');
  assert.equal(dollars(0.0002), '<$0.01');
  for (const value of [null, undefined]) {
    assert.equal(dollars(value), 'Not reported');
    assert.equal(count(value), 'Not reported');
  }
  for (const value of [-1, NaN, Infinity, '7.29']) assert.equal(dollars(value), 'Invalid reported cost');
  for (const value of [-1, 1.5, NaN, Infinity, '2', Number.MAX_SAFE_INTEGER + 1]) assert.equal(count(value), 'Invalid reported count');
  assert.equal(count(0), '0');
  assert.equal(fileSize(0), '0 bytes');
  assert.equal(fileSize(262144), '256 KiB');
  assert.equal(fileSize(-1), 'Invalid reported size');
});

test('cost coverage never conflates priced estimates with billed or unknown cost', () => {
  assert.match(costCoverage({ cost_complete: false }), /Partial coverage.*higher.*source unknown/);
  assert.match(costCoverage({ cost_complete: true, cost_basis: 'list_price' }), /All reported usage priced.*List-price estimate/);
  assert.match(costCoverage({ cost_basis: 'provider_reported' }), /coverage unknown.*Provider-reported/);
  assert.match(costCoverage({ cost_basis: 'mixed' }), /Mixed cost sources/);
  assert.match(costCoverage({}), /coverage unknown.*source unknown/);
});

test('dates and schedule cadence are readable without inventing missing information', () => {
  assert.equal(dateLabel(null), 'Not reported');
  assert.equal(dateLabel('invalid'), 'Invalid reported date');
  assert.equal(dateLabel(0), 'Invalid reported date');
  assert.match(dateLabel('2026-10-13T09:00:00Z'), /2026/);
  assert.equal(recurrenceLabel({ kind: 'interval', seconds: 3600 }), 'Every 1 hour');
  assert.equal(recurrenceLabel({ kind: 'interval', seconds: 90 }), 'Every 90 seconds');
  assert.equal(recurrenceLabel({ kind: 'interval', seconds: -1 }), 'Invalid reported interval');
  assert.equal(recurrenceLabel({ kind: 'cron', expression: '0 9 * * *', timezone: 'UTC' }), 'Daily at 09:00 · UTC');
  assert.equal(recurrenceLabel({ kind: 'cron', expression: '0 9 * * 1-5', timezone: 'UTC' }), 'Custom schedule · UTC');
  assert.equal(recurrenceLabel({ kind: 'cron', expression: '65 29 * * *' }), 'Custom schedule · timezone not reported');
  assert.equal(recurrenceLabel(null), 'Recurrence not reported');
  assert.throws(() => recurrenceLabel([]), /record/);
  assert.equal(scopeLabel(null, []), 'Scope not reported');
  assert.equal(scopeLabel({ scope: 'everywhere' }, []), 'Everywhere');
});
