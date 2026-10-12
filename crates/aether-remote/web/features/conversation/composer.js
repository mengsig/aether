import {button, errorText, invariant} from '../../ui/dom.js';
import {commandRoute, parseCommand} from './commands.js';
import {commandPalette} from './palette.js';
import {mentions} from './mentions.js';
import {draftView, finishedAgent, insertMention, resizeInput, saveDraft, updateRole} from './composer-input.js';

/** @typedef {import('../../types.js').Context} Context */
/** @typedef {import('./composer-input.js').ComposerState} ComposerState */

/** @param {Context} context @param {string} agentId @param {(name:string)=>void} local */
export function composer(context, agentId, local) {
  invariant(agentId.length > 0, 'Composer requires an agent');
  invariant(typeof context.store.getDraft === 'function', 'Composer requires draft persistence');
  const view = draftView(context, agentId, () => { void submit(state, context, agentId, execute); });
  const {root, input} = view;
  const state = {...view, busy:false, disposed:false};
  /** @param {import('./commands.js').SlashCommand} command @param {string} argument */
  function execute(command, argument) {
    palette.close(); files.close();
    if (input.value.startsWith('/')) { input.value = ''; saveDraft(state, context, agentId); resizeInput(input); updateRole(state, context, agentId); }
    if (command.route === 'local') { local(command.name); if (command.name === 'voice') input.focus(); return; }
    context.navigate(commandRoute(command, agentId, argument));
  }
  const palette = commandPalette(execute, () => input.focus());
  const persist = () => { saveDraft(state, context, agentId); resizeInput(input); updateRole(state, context, agentId); };
  const files = mentions(input, context, agentId, persist);
  const commands = button('/', () => { files.close(); palette.open(); }, 'button conversation-icon conversation-command-toggle');
  commands.setAttribute('aria-label', '/ Commands'); commands.setAttribute('aria-haspopup', 'dialog');
  const mention = button('@', () => { palette.close(); insertMention(input, () => { persist(); files.update(); }); }, 'button conversation-icon conversation-mention-toggle');
  mention.setAttribute('aria-label', 'Mention a file or agent');
  view.pill.append(input, commands, mention, view.send);
  root.append(palette.root, files.root);
  bindComposerInput(state, context, persist, execute, palette, files, agentId);
  updateRole(state, context, agentId);
  const layout = new ResizeObserver(() => resizeInput(input));
  layout.observe(view.pill);
  return {
    root, openCommands:() => { files.close(); palette.open(); },
    update() { updateRole(state, context, agentId); if (input.readOnly) files.close(); },
    /** @param {string} text */
    quote(text) {
      if (context.store.get().role !== 'operator') return;
      input.value += `${input.value ? '\n\n' : ''}${text.split('\n').map(line => `> ${line}`).join('\n')}\n\n`;
      persist(); input.focus();
    },
    destroy() { state.disposed = true; files.destroy(); layout.disconnect(); },
  };
}

/** @param {ComposerState} state @param {Context} context @param {()=>void} persist @param {(command:import('./commands.js').SlashCommand,argument:string)=>void} execute @param {ReturnType<typeof commandPalette>} palette @param {ReturnType<typeof mentions>} files @param {string} agentId */
function bindComposerInput(state, context, persist, execute, palette, files, agentId) {
  const {root, input} = state;
  invariant(root instanceof HTMLFormElement, 'Composer input requires a form');
  invariant(root.contains(input), 'Draft must belong to its composer');
  input.addEventListener('input', () => {
    persist();
    if (input.readOnly) return;
    files.update();
    if (/^\/[^\s]*$/.test(input.value)) palette.open(input.value, false);
    else palette.close();
  });
  input.addEventListener('click', () => { if (!input.readOnly) files.update(); });
  input.addEventListener('keydown', event => {
    if (event.key === 'Enter' && (event.ctrlKey || event.metaKey) && !event.isComposing) {
      event.preventDefault(); void submit(state, context, agentId, execute);
    }
    if (event.key === 'Escape') { palette.close(); files.close(); }
  });
  root.addEventListener('submit', event => { event.preventDefault(); void submit(state, context, agentId, execute); });
}

/** @param {ComposerState} state @param {Context} context @param {string} agentId @param {(command:import('./commands.js').SlashCommand,argument:string)=>void} execute */
async function submit(state, context, agentId, execute) {
  invariant(agentId.length > 0, 'Delivery requires an agent');
  invariant(typeof context.command === 'function', 'Delivery requires the authorized command boundary');
  const draft = state.input.value;
  if (state.busy || !draft.trim() || state.disposed) return;
  state.status.dataset.tone = 'error';
  state.status.textContent = '';
  if (context.store.get().role !== 'operator') { state.status.textContent = 'Observer access cannot send messages.'; return; }
  if (draft.startsWith('/')) {
    const parsed = parseCommand(draft);
    if (!parsed) { state.status.textContent = 'Unknown command. Open / Commands to see available actions.'; return; }
    execute(parsed.command, parsed.argument);
    return;
  }
  if (finishedAgent(context.store.get(), agentId)) { state.status.textContent = 'Restart this agent before sending. Your draft is saved.'; return; }
  if (context.store.get().connection !== 'online') { state.status.textContent = 'Reconnect before sending. Your draft is saved.'; return; }
  const key = crypto.randomUUID();
  state.busy = true;
  state.status.dataset.tone = 'quiet';
  state.status.textContent = 'Sending…';
  updateRole(state, context, agentId);
  try {
    context.store.setDraft(agentId, '');
    state.input.value = '';
    resizeInput(state.input);
    await context.command({cmd:'send_message', to:agentId, content:draft, idempotency_key:key});
    state.status.textContent = 'Delivered to the agent’s inbox.';
  } catch (error) {
    const receipt=context.store.get().outbox.find(item=>item.command.idempotency_key===key);
    let restored=true;
    if (!receipt && !state.input.value) {
      state.input.value = draft; restored=saveDraft(state, context, agentId); resizeInput(state.input);
    }
    state.status.dataset.tone = receipt ? 'quiet' : 'error';
    state.status.textContent = receipt ? '' : `Message not sent: ${errorText(error)}${restored ? '' : ' Your draft is in the text field but could not be saved on this device.'}`;
  } finally {
    state.busy = false;
    if (!state.disposed) updateRole(state, context, agentId);
  }
}
