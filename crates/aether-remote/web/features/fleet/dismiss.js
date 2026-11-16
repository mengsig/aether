import { button, errorText, invariant } from '../../ui/dom.js';

/** @typedef {import('../../types.js').Attention} Attention */
/** @typedef {import('../../types.js').Command} Command */
/** @typedef {import('../../types.js').Context} Context */
/** @typedef {import('../../types.js').State} State */
/** @typedef {{pending:Set<string>,errors:Map<string,string>,render:()=>void,alive:boolean}} Inbox */

/** @param {Attention['kind']} kind */
export function isDismissible(kind) {
  return kind === 'agent_failed' || kind === 'agent_blocked';
}

/** Only an operator may archive; observers never see the action. @param {State} state @param {Attention} item */
export function canDismiss(state, item) {
  return state.role === 'operator' && isDismissible(item.kind);
}

/** The failed agents shown in Attention; never running or blocked ones. @param {Attention[]} attention */
export function failedItems(attention) {
  invariant(Array.isArray(attention), 'Attention must be a list');
  const failed = attention.filter(item => item.kind === 'agent_failed');
  invariant(new Set(failed.map(item => item.agentId)).size === failed.length, 'Each failed agent appears once');
  return failed;
}

/** A blocked agent is still live, so it is stopped before it can be archived, as the TUI does. @param {Attention} item @returns {Command[]} */
export function dismissCommands(item) {
  invariant(isDismissible(item.kind), 'Only failed or blocked agents can be dismissed');
  invariant(item.agentId.length > 0, 'Dismissing requires an agent');
  const archive = {cmd: 'archive', target: {agent: item.agentId}, archived: true};
  if (item.kind === 'agent_failed') return [archive];
  return [{cmd: 'cancel', agent: item.agentId, reason: 'Dismissed from Attention'}, archive];
}

/** @param {Context} context @param {Attention} item */
function requireListed(context, item) {
  const state = context.store.get();
  invariant(state.role === 'operator', 'Observer access is read only');
  invariant(state.connection === 'online', 'Reconnect before dismissing');
  const current = state.attention.find(candidate => candidate.id === item.id);
  invariant(current && current.agentId === item.agentId, 'This agent no longer needs attention');
}

/** @param {Context} context @param {string} agentId */
function agentName(context, agentId) {
  return context.store.get().agents.find(agent => agent.id === agentId)?.name || agentId;
}

/** @param {Attention} item @param {Context} context @param {Inbox} inbox */
async function dismiss(item, context, inbox) {
  if (inbox.pending.has(item.id)) return;
  const name = agentName(context, item.agentId);
  const blocked = item.kind === 'agent_blocked';
  inbox.pending.add(item.id);
  inbox.errors.delete(item.id);
  inbox.render();
  try {
    const accepted = await context.confirm(blocked ? `Stop and dismiss ${name}?` : `Dismiss ${name}?`,
      blocked ? 'Ends this agent and its live descendants, then archives it. Its history stays available.'
        : 'Archives this failed agent. Its history stays available.');
    if (!accepted) return;
    requireListed(context, item);
    // Stopping a blocked agent drops its card, so the listing is checked once, up front.
    for (const command of dismissCommands(item)) await context.command(command);
    context.toast(`Dismissed ${name}`);
  } catch (error) {
    const text = errorText(error);
    inbox.errors.set(item.id, text);
    context.toast(`Could not dismiss ${name}: ${text}`);
  } finally {
    inbox.pending.delete(item.id);
    if (inbox.alive) inbox.render();
  }
}

/** @param {Attention} item @param {Context} context @param {Inbox} inbox @param {boolean} disabled */
export function dismissButton(item, context, inbox, disabled) {
  invariant(canDismiss(context.store.get(), item), 'Dismiss requires operator access and a failed or blocked agent');
  const action = button('Dismiss…', () => { void dismiss(item, context, inbox); }, 'button attention-deny');
  action.disabled = disabled;
  action.dataset.focus = `dismiss:${item.id}`;
  return action;
}

/** @param {Attention[]} targets @param {Context} context @param {Inbox} inbox */
async function archiveEach(targets, context, inbox) {
  let failures = 0;
  for (const item of targets) {
    try {
      requireListed(context, item);
      for (const command of dismissCommands(item)) await context.command(command);
    } catch (error) {
      failures += 1;
      inbox.errors.set(item.id, errorText(error));
    } finally {
      inbox.pending.delete(item.id);
    }
  }
  invariant(failures <= targets.length, 'Failures cannot exceed the attempted archives');
  return failures;
}

/** @param {Context} context @param {Inbox} inbox */
async function clearFailed(context, inbox) {
  const targets = failedItems(context.store.get().attention).filter(item => !inbox.pending.has(item.id));
  if (!targets.length) return;
  const count = targets.length;
  const noun = `failed agent${count === 1 ? '' : 's'}`;
  const accepted = await context.confirm(`Clear ${count} ${noun}?`,
    `Archives the ${count} ${noun} shown here. Running and blocked agents are not touched. History stays available.`);
  if (!accepted) return;
  for (const item of targets) { inbox.pending.add(item.id); inbox.errors.delete(item.id); }
  if (inbox.alive) inbox.render();
  const failures = await archiveEach(targets, context, inbox);
  if (inbox.alive) inbox.render();
  context.toast(failures ? `Could not clear ${failures} of ${count} ${noun}. See the cards for details.` : `Cleared ${count} ${noun}`);
}

/** @param {Context} context @param {Inbox} inbox @returns {HTMLButtonElement|null} */
export function clearFailedButton(context, inbox) {
  const state = context.store.get();
  if (state.role !== 'operator') return null;
  const failed = failedItems(state.attention);
  if (!failed.length) return null;
  const action = button(`Clear all failed (${failed.length})`, () => { void clearFailed(context, inbox); }, 'button attention-clear');
  action.disabled = state.connection !== 'online' || failed.some(item => inbox.pending.has(item.id));
  action.dataset.focus = 'clear-failed';
  return action;
}
