import { button, el, empty, errorText, heading, invariant, money, number } from '../../ui/dom.js';
import { array } from '../../core/validate.js';
import { unique } from '../../core/normalize.js';
import { applyEvent } from '../../core/events.js';
import { reported, usageSummary } from './model.js';
import { agentLink, connectionNotice, liveView, metric, timeLabel } from './shared.js';

/** @typedef {import('../../types.js').Context} Context */
/** @typedef {import('../../types.js').State} State */
/** @typedef {import('../../types.js').Agent} Agent */
/** @typedef {{label:string,tone:string,count:number}} StateGroup */
/** @typedef {{events:State['activity'],key:string,loaded:boolean,loading:boolean,error:string,alive:boolean}} History */

/** @param {Context} context @param {History} history @param {()=>void} render */
async function fetchHistory(context, history, render) {
  const state = context.store.get();
  invariant(state.ready && state.connection === 'online', 'History requires a ready connection');
  invariant(Number.isSafeInteger(state.lastSeq) && state.lastSeq >= 0, 'History requires a valid sequence');
  const key = history.key;
  history.loading = true;
  try {
    const response = await context.transport.request({cmd:'get_events',after:0,limit:200,query:{before:Number.MAX_SAFE_INTEGER}});
    if (response.reply !== 'events') throw new Error('Expected event history from the host');
    const records = array(response.data, 'Event history');
    if (records.length > 200) throw new Error('Event history exceeded the requested limit');
    const events = records.map(record => {
      const projection = applyEvent({...state,lastSeq:0,activity:[]},record);
      invariant(projection?.activity?.length === 1, 'History must project one observed event per record');
      return projection.activity[0];
    });
    if (history.alive && history.key === key) history.events = unique(events,event => event.seq);
  } catch (error) {
    if (history.alive && history.key === key) history.error = errorText(error);
  } finally {
    if (history.alive && history.key === key) {
      history.loading = false; history.loaded = true; render();
    }
  }
}

/** @param {State} state @param {History} history */
function withHistory(state, history) {
  const merged = new Map(history.events.map(event => [event.seq,event]));
  for (const event of state.activity) merged.set(event.seq,event);
  return {...state,activity:[...merged.values()].sort((left,right) => right.seq-left.seq).slice(0,500)};
}

/** @param {History} history @param {()=>void} retry */
function historyNotice(history, retry) {
  const notice = el('div', 'activity-history-notice');
  notice.setAttribute('role', 'status');
  notice.append(el('p', '', `Recent history could not load. ${history.error}`), button('Retry history', retry, 'button quiet'));
  return notice;
}

/** @param {Agent[]} agents @returns {StateGroup[]} */
function fleetGroups(agents) {
  invariant(agents.length > 0, 'Fleet state requires at least one agent');
  const groups = [
    {label:'Working',tone:'working',count:0}, {label:'Waiting',tone:'waiting',count:0},
    {label:'Complete',tone:'complete',count:0}, {label:'Failed',tone:'failed',count:0},
    {label:'Ready',tone:'ready',count:0}, {label:'Other',tone:'other',count:0},
  ];
  for (const agent of agents) {
    const status = agent.paused ? 'paused' : agent.status.toLowerCase();
    const index = ['running','working','active'].includes(status) ? 0
      : ['waiting','blocked','paused'].includes(status) ? 1
      : status === 'completed' ? 2 : status === 'failed' ? 3 : status === 'ready' ? 4 : 5;
    groups[index].count++;
  }
  invariant(groups.reduce((total, group) => total + group.count, 0) === agents.length, 'Every agent must be counted once');
  return groups.filter(group => group.count > 0);
}

/** @param {StateGroup[]} groups @param {number} total */
function stateRing(groups, total) {
  invariant(total > 0 && Number.isSafeInteger(total), 'State ring requires an agent count');
  invariant(groups.reduce((sum, group) => sum + group.count, 0) === total, 'Ring segments must match fleet total');
  const figure = el('div', 'activity-ring');
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 120 120');
  svg.setAttribute('role', 'img');
  svg.setAttribute('aria-label', `Fleet state: ${groups.map(group => `${group.count} ${group.label.toLowerCase()}`).join(', ')}`);
  let offset = 0;
  for (const group of groups) {
    const share = group.count / total * 100;
    const gap = groups.length > 1 ? Math.min(1.8, share * .12) : 0;
    const circle = document.createElementNS(svg.namespaceURI, 'circle');
    circle.setAttribute('cx', '60'); circle.setAttribute('cy', '60'); circle.setAttribute('r', '50');
    circle.setAttribute('pathLength', '100'); circle.setAttribute('fill', 'none');
    circle.setAttribute('stroke-width', '9');
    circle.setAttribute('stroke-dasharray', `${share-gap} ${100-share+gap}`);
    circle.setAttribute('stroke-dashoffset', String(-offset));
    circle.setAttribute('class', `activity-ring-segment activity-tone-${group.tone}`);
    svg.append(circle);
    offset += share;
  }
  const center = el('div', 'activity-ring-center');
  center.append(el('strong', '', String(total)), el('span', 'muted', total === 1 ? 'agent' : 'agents'));
  figure.append(svg, center);
  return figure;
}

/** @param {State} state */
function fleetPulse(state) {
  const groups = fleetGroups(state.agents);
  const section = el('section', 'activity-pulse');
  const header = el('div', 'activity-section-heading');
  const indicator = el('span', 'activity-live', state.connection === 'online' ? 'Live' : 'Last known state');
  indicator.dataset.online = String(state.connection === 'online');
  header.append(el('h2', '', 'Fleet pulse'), indicator);
  const body = el('div', 'activity-pulse-body');
  const legend = el('ul', 'activity-state-legend');
  legend.setAttribute('aria-label', 'Agent states');
  for (const group of groups) {
    const row = el('li', `activity-tone-${group.tone}`);
    row.append(el('span', 'activity-state-dot'), el('span', '', group.label), el('strong', '', String(group.count)));
    legend.append(row);
  }
  body.append(stateRing(groups, state.agents.length), legend);
  section.append(header, body, usageMetrics(state.agents));
  return section;
}

/** @param {Agent[]} agents */
function usageMetrics(agents) {
  const summary = usageSummary(agents);
  const metrics = el('dl', 'activity-totals');
  metrics.append(metric('Total tokens', number(summary.tokens)),
    metric('Total cost', summary.cost === null ? '—' : `${money(summary.cost)}${summary.costComplete ? '' : '*'}`));
  metrics.setAttribute('aria-describedby', 'activity-coverage');
  return metrics;
}

/** @param {Agent[]} agents */
function coverageNote(agents) {
  const summary = usageSummary(agents);
  const partial = summary.cost !== null && !summary.costComplete ? ' * Cost includes partial reporting.' : '';
  const note = el('p', 'activity-coverage',
    `Reported totals · tokens ${summary.tokenCoverage}/${summary.count} agents; cost ${summary.costCoverage}/${summary.count}.${partial} — means unreported.`);
  note.id = 'activity-coverage';
  return note;
}

/** @param {Agent[]} agents @param {Context} context @param {'tokens'|'cost'} measure @param {(measure:'tokens'|'cost')=>void} select */
function usageChart(agents, context, measure, select) {
  invariant(agents.length > 0, 'Usage chart requires agents');
  invariant(measure === 'tokens' || measure === 'cost', 'Usage chart measure must be supported');
  const chart = el('section', 'activity-usage');
  const header = el('div', 'activity-section-heading');
  header.append(el('h2', '', 'Usage by agent'));
  const switcher = el('div', 'activity-measures');
  switcher.setAttribute('role', 'group'); switcher.setAttribute('aria-label', 'Usage measure');
  for (const key of /** @type {const} */ (['tokens','cost'])) {
    const action = button(key === 'tokens' ? 'Tokens' : 'Cost', () => select(key), 'button');
    action.setAttribute('aria-pressed', String(key === measure));
    action.dataset.focus = `usage:${key}`;
    switcher.append(action);
  }
  header.append(switcher);
  const maximum = agents.reduce((max, agent) => Math.max(max, reported(agent[measure]) ? Number(agent[measure]) : 0), 0);
  const ranked = [...agents].sort((left, right) => (right[measure] ?? -1) - (left[measure] ?? -1));
  const list = el('ol', 'activity-bars');
  list.setAttribute('aria-label', measure === 'tokens' ? 'Tokens by agent' : 'Cost by agent');
  list.setAttribute('aria-describedby', 'activity-coverage');
  for (const agent of ranked) list.append(usageRow(agent, context, measure, maximum));
  chart.append(header, list, coverageNote(agents));
  return chart;
}

/** @param {Agent} agent @param {Context} context @param {'tokens'|'cost'} measure @param {number} maximum */
function usageRow(agent, context, measure, maximum) {
  invariant(Number.isFinite(maximum) && maximum >= 0, 'Chart maximum must be finite and nonnegative');
  const value = agent[measure];
  invariant(!reported(value) || Number(value) <= maximum, 'Usage cannot exceed the chart maximum');
  const row = el('li', 'activity-bar-row');
  const label = el('div', 'activity-bar-label');
  const formatted = reported(value) ? measure === 'tokens' ? Number(value).toLocaleString() : money(value) : '—';
  const suffix = measure === 'cost' && reported(value) && !agent.costComplete ? '*' : '';
  const amount = el('span', 'activity-bar-value', formatted + suffix);
  if (!reported(value)) amount.setAttribute('aria-label', 'Unreported');
  const link = agentLink(context, agent.id, agent.name || agent.id);
  link.dataset.focus = `usage-agent:${measure}:${agent.id}`;
  label.append(link, amount);
  const track = el('div', 'activity-bar-track');
  track.setAttribute('aria-hidden', 'true');
  track.dataset.reported = String(reported(value));
  const bar = el('div', 'activity-bar-fill');
  bar.style.width = `${maximum > 0 && reported(value) ? Number(value) / maximum * 100 : 0}%`;
  track.append(bar);
  row.append(label, track);
  return row;
}

/** @param {string} kind */
function eventPresentation(kind) {
  const known = /** @type {Record<string,[string,string,string]>} */ ({
    'question.asked':['Question asked','waiting','?'], 'approval.requested':['Approval requested','waiting','!'],
    'approval.resolved':['Approval resolved','neutral','✓'], 'agent.failed':['Agent failed','failed','!'],
    'agent.blocked':['Agent blocked','waiting','!'], 'agent.completed':['Work completed','working','✓'],
    'agent.started':['Work started','working','↗'], 'agent.spawned':['Agent started','working','↗'],
    'message.sent':['Message sent','neutral','↗'], 'agent.message':['Agent message','neutral','↗'],
  });
  return Object.hasOwn(known, kind) ? known[kind] : [kind.replaceAll('.', ' · ').replaceAll('_', ' '), 'neutral', '·'];
}

/** @param {import('../../types.js').Activity} event @param {Context} context @param {Map<string,string>} names */
function timelineEvent(event, context, names) {
  invariant(Number.isSafeInteger(event.seq) && event.seq > 0, 'Timeline events require a valid sequence');
  invariant(event.kind.length > 0, 'Timeline events require an observed kind');
  const [label, tone, glyph] = eventPresentation(event.kind);
  const row = el('li', `fleet-event activity-event activity-tone-${tone}`);
  row.tabIndex = -1;
  const marker = el('span', 'activity-event-marker', glyph);
  marker.setAttribute('aria-hidden', 'true');
  const body = el('div', 'activity-event-body');
  const meta = el('div', 'activity-event-meta');
  const time = timeLabel(event.at);
  time.title = time.textContent || '';
  time.setAttribute('aria-label', time.title);
  if (time.dateTime) time.textContent = new Date(event.at).toLocaleTimeString([], {hour:'2-digit',minute:'2-digit'});
  meta.append(el('span', 'activity-event-kind', label), time);
  body.append(meta);
  if (event.agentId) {
    const link = agentLink(context, event.agentId, names.get(event.agentId) || event.agentId);
    link.dataset.focus = `event-agent:${event.seq}:${event.agentId}`;
    body.append(link);
  }
  if (event.detail && event.detail !== event.kind) body.append(el('p', '', event.detail));
  row.append(marker, body);
  return row;
}

/** @param {State} state @param {Context} context @param {number} limit @param {History} history */
function eventTimeline(state, context, limit, history) {
  invariant(Number.isSafeInteger(limit) && limit > 0, 'Timeline limit must be positive');
  invariant(Array.isArray(state.activity), 'Timeline requires observed events');
  const section = el('section', 'activity-timeline');
  const header = el('div', 'activity-section-heading');
  header.append(el('h2', '', 'Event stream'), el('span', 'activity-event-count', String(state.activity.length)));
  const source = history.loading ? 'Loading recent history…' : history.loaded && !history.error
    ? 'Recent host history & live updates' : 'Received on this device';
  section.append(header, el('p', 'activity-timeline-caption', `${source} · newest first`));
  const events = [...state.activity].sort((left, right) => right.seq - left.seq).slice(0, limit);
  if (!events.length) {
    section.append(empty('Ready for the next update', 'Events appear here as your fleet works.'));
    return section;
  }
  const names = new Map(state.agents.map(agent => [agent.id, agent.name || agent.id]));
  const list = el('ol', 'activity-events');
  list.setAttribute('aria-label', 'Recent activity');
  for (const event of events) list.append(timelineEvent(event, context, names));
  section.append(list);
  return section;
}

/** @param {HTMLElement} container @param {Context} context */
export function mountActivity(container, context) {
  invariant(container instanceof HTMLElement, 'Activity requires a container');
  invariant(typeof context.store.get === 'function', 'Activity requires a state store');
  let limit = 50;
  /** @type {'tokens'|'cost'} */ let measure = 'tokens';
  /** @type {History} */ const history = {events:[],key:'',loaded:false,loading:false,error:'',alive:true};
  const render = () => {
    const focused = container.querySelector(':focus')?.getAttribute('data-focus');
    const current = context.store.get();
    const key = `${current.daemonId}:${current.role}`;
    if (history.key !== key) Object.assign(history,{events:[],key,loaded:false,loading:false,error:''});
    if (current.ready && current.connection === 'online' && !history.loaded && !history.loading) void fetchHistory(context,history,render);
    const state = withHistory(current, history);
    container.replaceChildren(heading('Activity & usage', 'The pulse of your fleet.'));
    const notice = connectionNotice(state, context);
    if (notice) container.append(notice);
    const body = el('div', 'activity-body');
    if (state.agents.length) {
      container.append(fleetPulse(state));
      body.append(usageChart(state.agents, context, measure, value => {
        measure = value; render();
        const selected = container.querySelector(`[data-focus="usage:${value}"]`);
        if (selected instanceof HTMLElement) selected.focus({preventScroll:true});
      }));
    } else if (state.connection === 'online') {
      const blank = empty('Usage will appear as work begins', 'There are no agents reporting usage yet.');
      blank.append(button('View fleet', () => context.navigate('fleet')));
      container.append(blank);
    }
    const timeline = eventTimeline(state, context, limit, history);
    if (history.error) timeline.append(historyNotice(history, () => { history.loaded = false; history.error = ''; render(); }));
    if (state.activity.length > limit) {
      const more = button(`Show more (${state.activity.length - limit} remaining)`, () => {
        const next = limit; limit += 50; render();
        const firstNew = container.querySelectorAll('.fleet-event')[next];
        if (firstNew instanceof HTMLElement) firstNew.focus();
      });
      more.dataset.focus = 'more-events';
      timeline.append(more);
    }
    body.append(timeline);
    container.append(body);
    const replacement = [...container.querySelectorAll('[data-focus]')].find(node => node.getAttribute('data-focus') === focused);
    if (replacement instanceof HTMLElement) replacement.focus({preventScroll:true});
  };
  const cleanup = liveView(container, context, render);
  return () => { history.alive = false; cleanup(); };
}
