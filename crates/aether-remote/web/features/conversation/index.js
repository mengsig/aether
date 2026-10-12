import {button, el, errorText, invariant} from '../../ui/dom.js';
import {composer} from './composer.js';
import {messageCard} from './cards.js';
import {copyText} from './markdown.js';
import {entryText} from './state.js';
import {transcriptSearch} from './search.js';
import {transcriptTail} from './tail.js';
import {virtualTranscript} from './virtual.js';
import {conversationHeader} from './header.js';
import {keyboardDock} from './chrome.js';

/** @param {HTMLElement} container @param {import('../../types.js').Context} context @param {string} agentId */
export function mount(container, context, agentId) {
  invariant(container instanceof HTMLElement, 'Conversation requires a container');
  invariant(typeof agentId === 'string' && agentId.length > 0, 'Conversation requires an agent id');
  const root = el('section', 'conversation-screen');
  const header = conversationHeader(context, agentId, () => search.focus());
  const viewport = el('div', 'conversation-viewport');
  viewport.tabIndex = 0;
  viewport.setAttribute('role', 'region');
  viewport.setAttribute('aria-label', 'Conversation transcript');
  const latest = button('Jump to latest ↓', () => virtual.latest(), 'button conversation-latest');
  latest.hidden = true;
  const notice = el('div', 'conversation-notice muted');
  const noticeText = el('span', '', 'Loading conversation…');
  const retry = button('Retry loading', () => { void loadConversation(context, agentId, state, schedule); });
  notice.append(noticeText, retry);
  notice.setAttribute('role', 'status');
  const state = {entries:/** @type {import('../../types.js').TranscriptEntry[]} */([]), hiddenBefore:-1, source:EMPTY_ENTRIES, cutoff:-2, frame:0, disposed:false, loaded:false, loadError:'', connection:context.store.get().connection};
  const compose = composer(context, agentId, name => localCommand(name, context, state, compose, search, schedule));
  const virtual = virtualTranscript(viewport, (entry, previous) => messageCard(entry, context, text => compose.quote(text), agentId, previous), following => { latest.hidden = following; });
  const search = transcriptSearch((index, query) => revealSearch(index, query, state.entries, virtual, viewport), () => {
    viewport.querySelector('.conversation-search-hit')?.classList.remove('conversation-search-hit'); viewport.focus();
  });
  const tail = transcriptTail(virtual.tail, context, agentId);
  root.append(header.root, search.root, notice, viewport, latest, compose.root);
  container.append(root);
  const releaseDock = keyboardDock(root, compose.root);
  function refresh() {
    state.frame = 0;
    const snapshot = context.store.get();
    const reconnected = state.connection !== 'online' && snapshot.connection === 'online';
    state.connection = snapshot.connection;
    if (reconnected && state.loadError && !state.loaded) void loadConversation(context, agentId, state, schedule);
    header.update(snapshot);
    const retained = snapshot.transcripts[agentId] ?? EMPTY_ENTRIES;
    if (retained !== state.source || state.hiddenBefore !== state.cutoff) {
      state.entries = state.hiddenBefore < 0 ? retained : retained.filter(entry => entry.seq > state.hiddenBefore);
      state.source = retained; state.cutoff = state.hiddenBefore;
    }
    const entries = state.entries;
    virtual.set(entries);
    search.set(entries);
    tail.update(snapshot);
    compose.update();
    noticeText.textContent = state.loadError || (!state.loaded ? 'Loading conversation…' : !entries.length ? 'No messages yet. Your conversation will appear here.' : '');
    notice.hidden = !noticeText.textContent;
    retry.hidden = !state.loadError;
  }
  function schedule() { if (!state.disposed && !state.frame) state.frame = requestAnimationFrame(refresh); }
  const unsubscribe = context.store.subscribe(schedule);
  refresh();
  void loadConversation(context, agentId, state, schedule);
  return () => {
    state.disposed = true; unsubscribe(); cancelAnimationFrame(state.frame);
    compose.destroy(); search.destroy(); virtual.destroy(); releaseDock(); root.remove();
  };
}

/** @type {import('../../types.js').TranscriptEntry[]} */
const EMPTY_ENTRIES = [];

/** @param {string} name @param {import('../../types.js').Context} context @param {{entries:import('../../types.js').TranscriptEntry[],hiddenBefore:number}} state @param {ReturnType<typeof composer>} compose @param {ReturnType<typeof transcriptSearch>} search @param {()=>void} schedule */
function localCommand(name, context, state, compose, search, schedule) {
  if (name === 'help') { compose.openCommands(); return; }
  if (name === 'search') { search.focus(); return; }
  if (name === 'voice') { context.toast('Tap the microphone on your phone keyboard, dictate into the draft, review, then Send.'); return; }
  if (name === 'image') { context.toast('Attaching an image from the phone is not available yet. In the terminal, use /image or aether send --image.'); return; }
  if (name === 'clear') {
    state.hiddenBefore = state.entries.reduce((maximum, entry) => Math.max(maximum, entry.seq), state.hiddenBefore);
    schedule();
    context.toast('Messages hidden for this visit. Reopen the conversation to restore them.');
    return;
  }
  if (name === 'copy') {
    const entry = [...state.entries].reverse().find(item => item.message.role === 'assistant');
    if (entry) void copyText(entryText(entry), context.toast);
    else context.toast('No agent reply to copy yet.');
  }
}

/** @param {number} index @param {string} query @param {import('../../types.js').TranscriptEntry[]} entries @param {{go:(index:number)=>void}} virtual @param {HTMLElement} viewport */
function revealSearch(index, query, entries, virtual, viewport) {
  invariant(Number.isInteger(index) && index >= 0 && index < entries.length, 'Search result must exist');
  invariant(viewport instanceof HTMLElement, 'Search requires the transcript viewport');
  virtual.go(index);
  requestAnimationFrame(() => {
    const row = viewport.querySelector(`[data-seq="${entries[index].seq}"]`);
    viewport.querySelectorAll('.conversation-search-hit').forEach(node => node.classList.remove('conversation-search-hit'));
    row?.classList.add('conversation-search-hit');
    row?.querySelectorAll('details.conversation-tool').forEach(details => { /** @type {HTMLDetailsElement} */(details).open = true; });
    if (!row?.querySelector('.conversation-content')?.textContent?.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())) {
      const provenance = row?.querySelector('.conversation-provenance');
      if (provenance instanceof HTMLDetailsElement) { provenance.hidden = false; provenance.open = true; }
    }
  });
}

/** @param {import('../../types.js').Context} context @param {string} agentId @param {{loaded:boolean,loadError:string}} state @param {()=>void} schedule */
async function loadConversation(context, agentId, state, schedule) {
  state.loadError = '';
  state.loaded = false;
  schedule();
  try { await context.store.loadTranscript(agentId); state.loaded = true; }
  catch (error) { state.loadError = `Could not load conversation: ${errorText(error)}`; }
  schedule();
}
