import {invariant} from '../../ui/dom.js';

export class HeightIndex {
  /** @param {number} count @param {number} [estimate] */
  constructor(count, estimate = 160) {
    invariant(Number.isInteger(count) && count >= 0, 'Invalid transcript row count');
    invariant(Number.isFinite(estimate) && estimate > 0, 'Invalid row estimate');
    this.heights = new Float64Array(count).fill(estimate);
    this.tree = new Float64Array(count + 1);
    for (let index = 1; index <= count; index++) this.tree[index] = (index & -index) * estimate;
  }

  /** @param {number} end */
  offset(end) {
    invariant(Number.isInteger(end) && end >= 0 && end <= this.heights.length, 'Offset index out of bounds');
    let total = 0;
    for (let index = end; index > 0; index -= index & -index) total += this.tree[index];
    invariant(Number.isFinite(total), 'Transcript offset overflow');
    return total;
  }

  /** @param {number} index @param {number} height */
  measure(index, height) {
    invariant(Number.isInteger(index) && index >= 0 && index < this.heights.length, 'Measured row out of bounds');
    invariant(Number.isFinite(height) && height > 0, 'Invalid measured height');
    const change = height - this.heights[index];
    this.heights[index] = height;
    for (let cursor = index + 1; cursor < this.tree.length; cursor += cursor & -cursor) this.tree[cursor] += change;
    return change;
  }

  /** @param {number} position */
  rowAt(position) {
    invariant(Number.isFinite(position), 'Invalid scroll position');
    const count = this.heights.length;
    if (count === 0) return 0;
    let index = 0;
    let sum = 0;
    let step = 1;
    while (step * 2 <= count) step *= 2;
    for (; step >= 1; step /= 2) {
      const next = index + step;
      if (next <= count && sum + this.tree[next] <= Math.max(0, position)) {
        sum += this.tree[next];
        index = next;
      }
    }
    invariant(index >= 0 && index <= count, 'Height lookup exceeded transcript');
    return Math.min(index, count - 1);
  }

  /** @param {number} top @param {number} viewport @param {number} [overscan] */
  range(top, viewport, overscan = 600) {
    invariant(Number.isFinite(viewport) && viewport >= 0, 'Invalid viewport height');
    invariant(Number.isFinite(overscan) && overscan >= 0, 'Invalid overscan');
    if (this.heights.length === 0) return {start:0, end:0};
    return {start:this.rowAt(top - overscan), end:Math.min(this.heights.length, this.rowAt(top + viewport + overscan) + 1)};
  }
}
