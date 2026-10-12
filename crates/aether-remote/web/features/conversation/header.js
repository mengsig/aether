import {button, el, invariant} from '../../ui/dom.js';
import {agentInbox} from './inbox.js';
import {chatButton} from './chrome.js';

/** @param {import('../../types.js').Context} context @param {string} agentId @param {()=>void} search */
export function conversationHeader(context, agentId, search) {
  invariant(agentId.length > 0, 'Conversation header requires an agent');
  invariant(typeof search === 'function', 'Conversation header requires search');
  const root = el('header', 'conversation-header');
  const identity = el('div', 'conversation-identity');
  const title = el('h1', '', 'Conversation');
  const status = el('p', 'connection conversation-connection');
  status.setAttribute('role', 'status');
  identity.append(title, status);
  const sheet = el('dialog', 'conversation-actions conversation-menu');
  sheet.setAttribute('aria-label', 'Conversation options');
  const model = el('p', 'muted small');
  const close = () => sheet.close();
  const top = el('div', 'conversation-sheet-heading');
  top.append(el('h2', '', 'Conversation'), chatButton('close', 'Close conversation options', close));
  sheet.append(top, model,
    button('Search conversation', () => { close(); search(); }, 'button conversation-menu-item'),
    button('Agent controls', () => { close(); context.navigate(`controls/${agentId}`); }, 'button conversation-menu-item'),
    agentInbox(context, agentId));
  const more = chatButton('more', 'Conversation options', () => sheet.showModal());
  more.setAttribute('aria-haspopup', 'dialog');
  sheet.addEventListener('click', event => { if (event.target === sheet && event.clientY < sheet.getBoundingClientRect().top) close(); });
  root.append(chatButton('back', 'Back to fleet', () => context.navigate('fleet')), identity, more, sheet);
  return {
    root,
    /** @param {import('../../types.js').State} snapshot */
    update(snapshot) {
      const agent = snapshot.agents.find(item => item.id === agentId);
      title.textContent = agent?.name || 'Conversation';
      const activity = agent?.paused ? 'Paused' : agent?.status || 'Connected';
      const connection = snapshot.connection;
      const label = connection === 'online' ? 'Connecting' : connection === 'reconnecting' ? 'Reconnecting…' : connection[0].toUpperCase() + connection.slice(1).replaceAll('-', ' ');
      status.dataset.state = connection;
      status.textContent = connection === 'online' && snapshot.ready !== false ? `Live · ${activity}` : `${label} · Your draft stays here`;
      status.title = snapshot.connectionDetail;
      model.textContent = agent ? `${agent.model} · ${agent.role}` : agentId;
    },
  };
}
