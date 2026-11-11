import {el, invariant} from '../../ui/dom.js';
import {HeightIndex} from './virtual-model.js';

/** @typedef {import('../../types.js').TranscriptEntry} Entry */

/** @param {HTMLElement} viewport @param {(entry:Entry,previous?:Entry)=>HTMLElement} render @param {(following:boolean)=>void} onFollow */
export function virtualTranscript(viewport, render, onFollow) {
  invariant(viewport instanceof HTMLElement, 'Transcript requires a viewport');
  invariant(typeof ResizeObserver === 'function', 'This browser needs ResizeObserver support');
  const top = el('div', 'conversation-spacer');
  const rows = el('div');
  const bottom = el('div', 'conversation-spacer');
  const tail = el('div', 'conversation-tail');
  viewport.append(top, rows, bottom, tail);
  const state = {entries:/** @type {Entry[]} */([]), heights:new HeightIndex(0), following:true, frame:0, width:0, disposed:false,programmaticTop:/** @type {number|null} */(null)};
  /** @type {Map<number,{entry:Entry,previous?:Entry,node:HTMLElement}>} */
  const nodes = new Map();
  /** @type {Map<number,number>} */
  const measured = new Map();
  const schedule = () => { if (!state.disposed && !state.frame) state.frame = requestAnimationFrame(draw); };
  const observer = new ResizeObserver(changes => {
    let adjustment = 0;
    for (const change of changes) {
      const index = Number(/** @type {HTMLElement} */(change.target).dataset.index);
      if (!change.target.isConnected || !Number.isInteger(index) || index >= state.entries.length) continue;
      if (/** @type {HTMLElement} */(change.target).dataset.seq !== String(state.entries[index].seq)) continue;
      const height = change.target.getBoundingClientRect().height;
      if (height <= 0 || Math.abs(state.heights.heights[index] - height) < 0.5) continue;
      const above = state.heights.offset(index + 1) <= viewport.scrollTop;
      const delta = state.heights.measure(index, height);
      measured.set(state.entries[index].seq, height);
      if (above && !state.following) adjustment += delta;
    }
    if (adjustment) viewport.scrollTop += adjustment;
    schedule();
  });
  function draw() {
    state.frame = 0;
    const position = state.following ? Math.max(0, state.heights.offset(state.entries.length) - viewport.clientHeight) : viewport.scrollTop;
    const range = state.heights.range(position, viewport.clientHeight);
    reconcileRows(rows, state.entries, range, nodes, observer, render);
    top.style.height = `${state.heights.offset(range.start)}px`;
    bottom.style.height = `${state.heights.offset(state.entries.length) - state.heights.offset(range.end)}px`;
    if (state.following) { alignLatest(viewport, rows); state.programmaticTop = viewport.scrollTop; }
  }
  const scroll = () => {
    if (state.programmaticTop !== null && Math.abs(viewport.scrollTop-state.programmaticTop)<1) { schedule(); return; }
    state.programmaticTop = null;
    state.following = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight < 80;
    onFollow(state.following);
    schedule();
  };
  viewport.addEventListener('scroll', scroll, {passive:true});
  const layout = new ResizeObserver(() => {
    if (state.width !== viewport.clientWidth) resetMeasurements(state, viewport, measured, nodes, observer);
    schedule();
  });
  layout.observe(viewport);
  layout.observe(tail);
  return virtualAPI(state, viewport, tail, measured, schedule, scroll, observer, layout, onFollow);
}

/** @param {HTMLElement} viewport @param {HTMLElement} rows */
function alignLatest(viewport, rows) {
  invariant(viewport.contains(rows), 'Latest alignment requires mounted transcript rows');
  const oldInset = Number(viewport.dataset.latestInset ?? 0);
  invariant(Number.isSafeInteger(oldInset) && oldInset >= 0, 'Latest inset must be a nonnegative pixel count');
  viewport.scrollTop = viewport.scrollHeight;
  const edge = viewport.getBoundingClientRect().top;
  let inset = 0;
  for (const chip of rows.querySelectorAll('.conversation-tool:not([open]) > summary')) {
    const box = chip.getBoundingClientRect();
    const top = box.top + oldInset; const bottom = box.bottom + oldInset;
    if (top < edge + 8 && bottom > edge) inset = Math.ceil(bottom - edge + 8);
  }
  viewport.dataset.latestInset = String(inset);
  viewport.style.setProperty('--conversation-latest-inset', `${inset}px`);
  viewport.scrollTop = viewport.scrollHeight;
}

/** @param {HTMLElement} rows @param {Entry[]} entries @param {{start:number,end:number}} range @param {Map<number,{entry:Entry,previous?:Entry,node:HTMLElement}>} nodes @param {ResizeObserver} observer @param {(entry:Entry,previous?:Entry)=>HTMLElement} render */
function reconcileRows(rows, entries, range, nodes, observer, render) {
  invariant(rows instanceof HTMLElement, 'Virtual rows require their mount point');
  invariant(range.start >= 0 && range.end <= entries.length && range.start <= range.end, 'Visible range must stay inside the transcript');
  const visible = new Set(entries.slice(range.start, range.end).map(entry => entry.seq));
  for (const [key, item] of nodes) {
    if (visible.has(key)) continue;
    observer.unobserve(item.node);
    item.node.remove();
    nodes.delete(key);
  }
  let before = rows.firstChild;
  for (let index = range.start; index < range.end; index++) {
    const entry = entries[index];
    let item = nodes.get(entry.seq);
    if (item && (item.entry !== entry || item.previous !== entries[index - 1])) {
      observer.unobserve(item.node);
      if (before === item.node) before = item.node.nextSibling;
      item.node.remove();
      nodes.delete(entry.seq);
      item = undefined;
    }
    if (!item) {
      item = {entry, previous:entries[index - 1], node:render(entry, entries[index - 1])};
      nodes.set(entry.seq, item);
      item.node.dataset.index = String(index);
      observer.observe(item.node);
    }
    item.node.dataset.index = String(index);
    if (item.node !== before) rows.insertBefore(item.node, before);
    before = item.node.nextSibling;
  }
}

/** @typedef {{entries:Entry[],heights:HeightIndex,following:boolean,frame:number,width:number,disposed:boolean,programmaticTop:number|null}} VirtualState */
/** @param {VirtualState} state @param {HTMLElement} viewport @param {HTMLElement} tail @param {Map<number,number>} measured @param {()=>void} schedule @param {()=>void} scroll @param {ResizeObserver} observer @param {ResizeObserver} layout @param {(following:boolean)=>void} onFollow */
function virtualAPI(state, viewport, tail, measured, schedule, scroll, observer, layout, onFollow) {
  return {
    tail,
    /** @param {Entry[]} entries */
    set(entries) {
      if (entries === state.entries) return;
      invariant(new Set(entries.map(entry => entry.seq)).size === entries.length, 'Transcript sequence keys must be unique');
      invariant(entries.every(entry => Number.isSafeInteger(entry.seq) && entry.seq >= 0), 'Transcript sequence keys must be nonnegative integers');
      const oldIndex = state.heights.rowAt(viewport.scrollTop);
      const anchor = state.entries[oldIndex]?.seq;
      const offset = viewport.scrollTop - state.heights.offset(oldIndex);
      state.entries = entries;
      state.heights = new HeightIndex(entries.length);
      entries.forEach((entry, index) => {
        const height = measured.get(entry.seq);
        if (height) state.heights.measure(index, height);
      });
      if (!state.following && anchor !== undefined) {
        const index = entries.findIndex(entry => entry.seq === anchor);
        if (index >= 0) viewport.scrollTop = state.heights.offset(index) + offset;
      }
      schedule();
    },
    latest() { state.following = true; onFollow(true); schedule(); },
    /** @param {number} index */
    go(index) {
      invariant(Number.isInteger(index) && index >= 0 && index < state.entries.length, 'Search match is outside transcript');
      state.following = false;
      onFollow(false);
      viewport.scrollTop = state.heights.offset(index);
      schedule();
    },
    destroy() {
      state.disposed = true;
      cancelAnimationFrame(state.frame);
      observer.disconnect();
      layout.disconnect();
      viewport.removeEventListener('scroll', scroll);
    },
  };
}

/** @param {VirtualState} state @param {HTMLElement} viewport @param {Map<number,number>} measured @param {Map<number,{entry:Entry,previous?:Entry,node:HTMLElement}>} nodes @param {ResizeObserver} observer */
function resetMeasurements(state, viewport, measured, nodes, observer) {
  const anchor = state.heights.rowAt(viewport.scrollTop);
  const within = viewport.scrollTop - state.heights.offset(anchor);
  state.width = viewport.clientWidth;
  measured.clear();
  state.heights = new HeightIndex(state.entries.length);
  if (!state.following) viewport.scrollTop = state.heights.offset(anchor) + Math.min(within, 159);
  for (const value of nodes.values()) { observer.unobserve(value.node); observer.observe(value.node); }
}
