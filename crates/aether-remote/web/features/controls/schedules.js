import { el } from '../../ui/dom.js';
import { panel, card, form, action, details, readList, replaceRows } from './ui.js';
import { required, record, scheduleCommand } from './protocol.js';
import {spawnScheduleForm} from './spawn-schedule.js';
import { count, dateLabel, recurrenceLabel, words } from './format.js';
import { rawDetails } from './presentation.js';

/** @typedef {import('./ui.js').Panel} Panel */

/** @param {Panel} view @param {string} agent @param {()=>Promise<void>} refresh */
function createForm(view, agent, refresh) {
  return form(view, 'Create schedule', [
    { label: 'Schedule name', name: 'name', required: true },
    { label: 'Recurrence', name: 'kind', options: ['interval', 'cron'] },
    { label: 'Interval in seconds (interval only)', name: 'seconds', type: 'number', value: '3600' },
    { label: 'Five-field cron expression (cron only)', name: 'expression', value: '0 9 * * *' },
    { label: 'IANA timezone (cron only)', name: 'timezone', value: Intl.DateTimeFormat().resolvedOptions().timeZone },
    { label: 'Message for the agent', name: 'message', type: 'textarea', required: true },
    { label: 'When the agent is busy', name: 'busy', options: ['queue_one', 'skip'] },
    { label: 'Missed occurrences', name: 'missed', options: ['coalesce', 'skip'] },
    { label: 'Maximum deliveries (blank for unlimited)', name: 'limit', type: 'number' },
  ], async values => {
    const result = await view.change(scheduleCommand(values, agent));
    if (result) await refresh();
  });
}

/** @param {Panel} view @param {Record<string,unknown>} schedule @param {()=>Promise<void>} refresh */
function scheduleRow(view, schedule, refresh) {
  const id = required(schedule.id, 'Schedule ID');
  const status = record(schedule.status);
  const root = card(required(schedule.name, 'Schedule name'), recurrenceLabel(schedule.recurrence));
  root.append(scheduleSummary(schedule, status, view.context.store.get().agents), rawDetails(schedule));
  const actions = el('div', 'row controls-actions');
  const history = el('div', 'stack');
  actions.append(action(view, 'History', async () => {
    const entries = await readList(view, { cmd: 'schedule_history', id, limit: 30 }, 'schedule_history');
    if (view.alive()) replaceRows(history, entries.map(occurrenceRow), 'No occurrences recorded.');
  }));
  for (const operation of ['pause', 'resume', 'cancel']) {
    if (operation === 'pause' && status.state !== 'active') continue;
    if (operation === 'resume' && status.state !== 'paused') continue;
    if (operation === 'cancel' && ['cancelled', 'exhausted'].includes(String(status.state))) continue;
    actions.append(action(view, operation[0].toUpperCase() + operation.slice(1), async () => {
      const result = await view.change({ cmd: 'update_schedule', id, action: operation },
        operation === 'cancel' ? `Cancel schedule ${schedule.name}? No further occurrences will be delivered.` : undefined);
      if (result) await refresh();
    }, true, operation === 'cancel'));
  }
  root.append(actions, history);
  return root;
}

/** @param {Record<string,unknown>} schedule @param {Record<string,unknown>} status @param {import('../../types.js').Agent[]} agents */
function scheduleSummary(schedule, status, agents) {
  const root = el('div', 'stack');
  const target = agents.find(agent => agent.id === schedule.target);
  const spawn = schedule.spawn == null ? null : record(schedule.spawn);
  const summary = { Status: words(status.state), Target: spawn ? 'Start a new run' : target?.name || 'Agent not in live view',
    Deliveries: count(schedule.deliveries), Limit: schedule.max_deliveries === null ? 'Unlimited' : count(schedule.max_deliveries) };
  root.append(details(summary));
  if (status.state === 'active') root.append(details({ 'Next delivery': dateLabel(status.next_due) }));
  if (status.state === 'paused') root.append(el('p', 'muted', 'Paused · no deliveries until resumed.'));
  if (typeof status.error === 'string') root.append(el('p', 'controls-error', status.error));
  const message = spawn ? spawn.objective : schedule.payload;
  if (typeof message === 'string') root.append(el('p', 'controls-prose', message));
  return root;
}

/** @param {Record<string,unknown>} entry */
function occurrenceRow(entry) {
  const outcome = record(entry.outcome);
  const root = el('section', 'stack controls-resource');
  root.append(details({ Outcome: words(outcome.outcome), Due: dateLabel(entry.due), Recorded: dateLabel(entry.recorded_at) }));
  if (typeof outcome.error === 'string') root.append(el('p', 'controls-error', outcome.error));
  if (typeof outcome.reason === 'string') root.append(el('p', 'muted', outcome.reason));
  root.append(el('p', 'muted controls-caption', 'Delivery history does not confirm that the work completed.'), rawDetails(entry));
  return root;
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context @param {string} agent */
export function mountSchedules(container, context, agent) {
  required(agent, 'Target agent');
  const view = panel(container, context);
  const list = el('div', 'stack');
  const refresh = async () => {
    const schedules = await readList(view, { cmd: 'list_schedules' }, 'schedules');
    if (!view.alive()) return;
    replaceRows(list, schedules.map(schedule => scheduleRow(view, schedule, refresh)), 'No schedules in this fleet.');
    view.sync();
  };
  view.body.append(card('Schedules', 'Recurring work across your fleet. Send to this agent or schedule fresh runs.'),
    action(view, 'Refresh schedules', refresh), list, createForm(view, agent, refresh),spawnScheduleForm(view,refresh));
  view.sync();
  void view.run(refresh);
  return view.dispose;
}
