/** @param {unknown} value @param {import('../types.js').Agent[]} [agents] */
export function actorLabel(value, agents = []) {
  if (value == null) return 'Not reported';
  if (typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid actor provenance');
  const actor = /** @type {Record<string,unknown>} */ (value);
  if (actor.actor === 'user') {
    const name = typeof actor.name === 'string' && actor.name ? ` ${actor.name}` : '';
    const origin = actor.origin && typeof actor.origin === 'object' ? /** @type {Record<string,unknown>} */ (actor.origin) : null;
    const device = origin?.transport === 'device' && typeof origin.name === 'string' ? ` · ${origin.name}` : '';
    return `Operator${name}${device}`;
  }
  if (actor.actor === 'agent' && typeof actor.id === 'string') return `Agent ${agents.find(agent => agent.id === actor.id)?.name || actor.id}`;
  if (actor.actor === 'tool' && typeof actor.call === 'string') return `Tool call ${actor.call}`;
  if (actor.actor === 'rule' && typeof actor.rule === 'string') return `Approval rule ${actor.rule}`;
  if (actor.actor === 'system') return 'System';
  if (actor.actor === 'scheduler') return 'Scheduler';
  if (actor.actor === 'client' && typeof actor.id === 'string') return `Client ${actor.id}`;
  if (actor.actor === 'imported' && actor.was && typeof actor.was === 'object') return 'Imported actor · see original delivery';
  throw new Error('Unknown actor provenance');
}
