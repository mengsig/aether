import test from 'node:test';
import assert from 'node:assert/strict';
import {HeightIndex} from './virtual-model.js';

test('variable measurements update offsets and binary row lookup', () => {
  const index = new HeightIndex(4, 100);
  assert.equal(index.measure(1, 240), 140);
  assert.equal(index.measure(3, 60), -40);
  assert.deepEqual([0,1,2,3,4].map(value => index.offset(value)), [0,100,340,440,500]);
  assert.equal(index.rowAt(99), 0);
  assert.equal(index.rowAt(100), 1);
  assert.equal(index.rowAt(339), 1);
  assert.equal(index.rowAt(340), 2);
  assert.equal(index.rowAt(500), 3);
  assert.equal(index.rowAt(-20), 0);
});

test('many varying heights agree with a linear reference model', () => {
  const count = 2053;
  const index = new HeightIndex(count);
  const heights = Array.from({length:count}, (_, row) => 20 + ((row * 71) % 511));
  heights.forEach((height, row) => index.measure(row, height));
  let offset = 0;
  for (let row = 0; row < count; row++) {
    assert.equal(index.offset(row), offset);
    assert.equal(index.rowAt(offset), row);
    assert.equal(index.rowAt(offset + heights[row] - 0.5), row);
    offset += heights[row];
  }
  assert.equal(index.offset(count), offset);
});

test('100,000 transcript entries mount a bounded range around the viewport', () => {
  const index = new HeightIndex(100_000, 120);
  const range = index.range(6_000_000, 800, 600);
  assert.ok(range.start > 49_000);
  assert.ok(range.end - range.start < 20);
  assert.ok(index.offset(range.start) <= 6_000_000);
  assert.ok(index.offset(range.end) >= 6_000_800);
});

test('empty, last row, and invalid measurements have explicit behavior', () => {
  const empty = new HeightIndex(0);
  assert.deepEqual(empty.range(0, 800), {start:0,end:0});
  assert.equal(empty.offset(0), 0);
  assert.equal(empty.rowAt(10), 0);
  assert.throws(() => empty.measure(0, 10), /bounds/);
  assert.throws(() => new HeightIndex(-1), /count/);
  const index = new HeightIndex(3);
  assert.throws(() => index.measure(1, NaN), /height/);
  assert.throws(() => index.measure(1, 0), /height/);
  assert.throws(() => index.rowAt(Infinity), /position/);
  assert.throws(() => index.range(0, -1), /viewport/);
  assert.deepEqual(index.range(480, 100, 0), {start:2,end:3});
});
