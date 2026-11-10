import { invariant } from '../../ui/dom.js';

/** @typedef {import('../../types.js').Agent} Agent */
/** @typedef {import('../../types.js').Attention} Attention */
/** @typedef {{agent:Agent,depth:number,parentMissing:boolean,cycle:boolean}} FleetRow */

/** @param {Agent[]} agents @returns {FleetRow[]} */
export function fleetRows(agents) {
  invariant(Array.isArray(agents), 'Fleet must be an array');
  const byId = new Map(agents.map(agent => [agent.id, agent]));
  invariant(byId.size === agents.length, 'Agent IDs must be unique');
  /** @type {Map<string, Agent[]>} */
  const children = new Map();
  const roots = [];
  for (const agent of agents) {
    invariant(agent.id.length > 0, 'An agent requires an ID');
    if (!agent.parent || !byId.has(agent.parent)) roots.push(agent);
    else {
      if (!children.has(agent.parent)) children.set(agent.parent, []);
      children.get(agent.parent)?.push(agent);
    }
  }
  /** @type {FleetRow[]} */
  const rows = [];
  const visited = new Set();
  for (const root of [...roots, ...agents]) {
    if (visited.has(root.id)) continue;
    const cycle = Boolean(root.parent && byId.has(root.parent));
    const pending = [{ agent: root, depth: 0 }];
    while (pending.length) {
      const current = pending.pop();
      invariant(current, 'A pending tree row must exist');
      if (visited.has(current.agent.id)) continue;
      visited.add(current.agent.id);
      rows.push({ ...current, cycle, parentMissing: Boolean(current.agent.parent && !byId.has(current.agent.parent)) });
      const descendants = children.get(current.agent.id) || [];
      for (let index = descendants.length - 1; index >= 0; index--) {
        pending.push({ agent: descendants[index], depth: current.depth + 1 });
      }
    }
  }
  invariant(rows.length === agents.length, 'Every agent must appear exactly once');
  return rows;
}

/** @param {number|null} value */
export function reported(value) {
  return value !== null && Number.isFinite(value) && value >= 0;
}

/** @param {Agent[]} agents */
export function usageSummary(agents) {
  invariant(Array.isArray(agents), 'Usage requires an agent array');
  let tokens = 0;
  let cost = 0;
  let tokenCoverage = 0;
  let costCoverage = 0;
  for (const agent of agents) {
    if (reported(agent.tokens)) { tokens += Number(agent.tokens); tokenCoverage++; }
    if (reported(agent.cost)) { cost += Number(agent.cost); costCoverage++; }
  }
  invariant(Number.isFinite(tokens) && Number.isFinite(cost), 'Usage totals must be finite');
  return { tokens: tokenCoverage ? tokens : null, cost: costCoverage ? cost : null,
    tokenCoverage, costCoverage, count: agents.length,
    costComplete: agents.length > 0 && agents.every(agent => reported(agent.cost) && agent.costComplete) };
}

/** @param {string} timestamp @param {number} [now] */
export function elapsed(timestamp, now = Date.now()) {
  const started = Date.parse(timestamp);
  if (!Number.isFinite(started) || !Number.isFinite(now) || started > now) return 'Not reported';
  const minutes = Math.floor((now - started) / 60000);
  if (minutes < 1) return 'Less than a minute';
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

/** @param {Attention} item */
export function approvalDetails(item) {
  invariant(item.kind === 'approval', 'Expected an approval');
  invariant(item.raw && typeof item.raw === 'object', 'Approval must retain its original record');
  const raw = item.raw;
  return {
    tool: typeof raw.tool === 'string' ? raw.tool : 'Not reported',
    argument: typeof raw.argument === 'string' ? raw.argument : 'Not reported by daemon',
    resources: raw.resources === undefined ? 'Not reported by daemon' : JSON.stringify(raw.resources, null, 2),
    effect: typeof raw.side_effect === 'string' ? raw.side_effect : 'Not reported by daemon',
    original: JSON.stringify(raw, null, 2),
  };
}

/** @param {Attention} item */
export function approvalHeadline(item) {
  const {tool} = approvalDetails(item);
  const name = tool.toLowerCase().split(/[/:.]/).at(-1) || '';
  if (name === 'git_push') return 'Push a branch';
  if (name === 'git_commit') return 'Commit workspace changes';
  if (['web_fetch','fetch','http_request'].includes(name)) return 'Fetch a web page';
  if (['bash','shell','run','exec','exec_command','run_command'].includes(name)) return 'Run a command';
  if (['write_file','edit_file','apply_patch','write','edit'].includes(name)) return 'Change files';
  if (['read_file','read','list_directory','glob','search_files'].includes(name)) return 'Read workspace files';
  if (['send_message','send','reply_to_question'].includes(name)) return 'Send a message';
  if (['delegate','create_agent'].includes(name)) return 'Start an agent';
  return 'Run a tool';
}

/** @param {Attention} item @param {'approved'|'denied'} decision @returns {import('../../types.js').Command} */
export function approvalCommand(item, decision) {
  invariant(item.kind === 'approval' && item.id.length > 0, 'Approval requires a valid ID');
  invariant(decision === 'approved' || decision === 'denied', 'Invalid approval decision');
  return { cmd: 'resolve_approval', id: item.id, decision };
}

/** @param {Attention} item @param {string} content @returns {import('../../types.js').Command} */
export function questionCommand(item, content) {
  invariant(item.kind === 'question' && item.agentId.length > 0, 'Question requires an agent');
  invariant(Number.isSafeInteger(item.questionSeq) && Number(item.questionSeq) > 0, 'Question sequence is missing or invalid');
  invariant(content.trim().length > 0, 'Write a reply before sending');
  return { cmd: 'reply_to_question', to: item.agentId, question_seq: item.questionSeq, content };
}
