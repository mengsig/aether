import { invariant } from '../../ui/dom.js';

/** @typedef {import('../../types.js').Context} Context */
/** @typedef {import('../../types.js').Command} Command */
/** @typedef {Record<string, unknown>} RecordData */

/** @param {unknown} value @returns {RecordData} */
export function record(value) {
  invariant(value !== null && typeof value === 'object', 'Expected an object from the daemon.');
  invariant(!Array.isArray(value), 'Expected a record, received a list.');
  return /** @type {RecordData} */ (value);
}

/** @param {unknown} value @param {string} label */
export function required(value, label) {
  invariant(typeof value === 'string', `${label} must be text.`);
  invariant(value.trim().length > 0, `${label} is required.`);
  return value.trim();
}

/** @param {import('../../types.js').Reply} reply @param {string} expected */
export function replyData(reply, expected) {
  invariant(reply.reply === expected, `Expected ${expected}, received ${reply.reply}.`);
  invariant(reply.data !== undefined, `The ${expected} response has no data.`);
  return reply.data;
}

/** @param {unknown} value */
export function records(value) {
  invariant(Array.isArray(value), 'Expected a list from the daemon.');
  return value.map(record);
}

/** @param {Context} context */
export function assertOperator(context) {
  const state = context.store.get();
  invariant(state.role === 'operator', 'Observer devices are read only.');
  invariant(state.connection === 'online', 'Reconnect before changing daemon state.');
}

/** @param {Context} context */
export function assertDeviceSelf(context) {
  const state = context.store.get();
  invariant(['operator', 'observer'].includes(state.role), 'A paired device is required.');
  invariant(state.connection === 'online', 'Reconnect before changing notification subscriptions.');
}

/** @param {Context} context @param {Command} command @param {string} [confirmation] */
export async function mutate(context, command, confirmation) {
  assertOperator(context);
  required(command.cmd, 'Command');
  if (confirmation && !await context.confirm('Confirm change', confirmation)) return null;
  assertOperator(context);
  return context.command(command);
}

/** @param {string} value */
export function modelReference(value) {
  const model = required(value, 'Model');
  const slash = model.indexOf('/');
  invariant(slash > 0 && slash < model.length - 1, 'Use a provider/model name.');
  return { provider: model.slice(0, slash), model: model.slice(slash + 1) };
}

/** @param {string} value @param {string} label @param {number} [minimum] */
export function integer(value, label, minimum = 1) {
  invariant(/^\d+$/.test(value), `${label} must be a whole number.`);
  const result = Number(value);
  invariant(Number.isSafeInteger(result) && result >= minimum, `${label} must be at least ${minimum}.`);
  return result;
}

/** @param {Record<string,string>} values @param {string} agent @returns {Command} */
export function scheduleCommand(values, agent) {
  required(agent, 'Target agent');
  invariant(['interval', 'cron'].includes(values.kind), 'Choose an interval or cron schedule.');
  const recurrence = values.kind === 'interval'
    ? { kind: 'interval', seconds: integer(values.seconds, 'Interval seconds') }
    : { kind: 'cron', expression: required(values.expression, 'Cron expression'), timezone: required(values.timezone, 'Timezone') };
  if (recurrence.kind === 'cron') {
    invariant(recurrence.expression?.split(/\s+/).length === 5, 'Cron expressions need five fields.');
    new Intl.DateTimeFormat('en', { timeZone: recurrence.timezone });
  }
  invariant(['skip', 'queue_one'].includes(values.busy), 'Invalid busy policy.');
  invariant(['coalesce', 'skip'].includes(values.missed), 'Invalid missed-run policy.');
  return { cmd: 'create_schedule', spec: {
    name: required(values.name, 'Schedule name'), target: agent, recurrence,
    payload: values.message, busy: values.busy, missed: values.missed,
    max_deliveries: values.limit ? integer(values.limit, 'Maximum deliveries') : null,
  } };
}

/** @param {Record<string,string>} values @param {string} agent @returns {Command} */
export function memoryCommand(values, agent) {
  invariant(['agent', 'repository', 'global'].includes(values.scope), 'Invalid memory scope.');
  invariant(['fact', 'decision', 'lesson', 'constraint', 'preference'].includes(values.kind), 'Invalid memory kind.');
  const scope = values.scope === 'agent' ? { scope: 'agent', id: required(agent, 'Agent') }
    : values.scope === 'repository' ? { scope: 'repository', root: required(values.root, 'Repository root') }
      : { scope: 'global' };
  return { cmd: 'create_memory', spec: { text: required(values.text, 'Memory'), kind: values.kind,
    scope, tags: values.tags.split(',').map(tag => tag.trim()).filter(Boolean) } };
}

/** @param {Record<string,string>} values @param {string} agent @returns {Command} */
export function ruleCommand(values, agent) {
  invariant(['agent', 'subtree', 'run'].includes(values.scope), 'Browser rules must target an agent, subtree or run. Everywhere rules require host access.');
  invariant(['approved', 'denied'].includes(values.decision), 'Invalid rule decision.');
  const scope = values.scope === 'agent' ? { scope: 'agent', id: required(agent, 'Agent') }
    : values.scope === 'subtree' ? { scope: 'subtree', root: required(agent, 'Agent') }
      : { scope: 'run', id: required(values.run, 'Run') };
  return { cmd: 'add_approval_rule', scope, decision: values.decision, note: values.note || null,
    match: { tool: values.tool.trim() || null, argument: values.argument.trim() || null,
      resource_prefix: values.resource.trim() || null } };
}

/** @param {unknown} value */
export function artifactBytes(value) {
  const data = record(value);
  const artifact = record(data.artifact);
  invariant(typeof data.base64 === 'string', 'Artifact bytes are missing.');
  invariant(typeof data.truncated === 'boolean', 'Artifact truncation status is missing.');
  const binary = atob(data.base64);
  invariant(binary.length <= 262144, 'Artifact response exceeds the requested limit.');
  return { artifact, truncated: data.truncated, bytes: Uint8Array.from(binary, char => char.charCodeAt(0)) };
}
