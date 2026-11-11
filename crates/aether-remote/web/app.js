import {createTransport, createStore, createCommand} from './core/index.js';
import {createShell} from './ui/shell.js';
import {createConfirm, createToast} from './ui/feedback.js';
import {readPreferences, savePreference} from './ui/preferences.js';
import {syncNotifications} from './ui/notification-sync.js';
import {el, errorText, invariant} from './ui/dom.js';
import {mountFleet, mountAttention, mountActivity} from './features/fleet/index.js';
import {mount as mountConversation} from './features/conversation/index.js';
import {mountControls, mountSettings} from './features/controls/index.js';
import {mountPair, takePairingSecret} from './features/start/pair.js';
import {mountNew} from './features/start/new.js';

const host = document.getElementById('app');
invariant(host instanceof HTMLElement, 'App root is missing');
const feedback = el('div'); document.body.append(feedback);
const toast = createToast(feedback);
const transport = createTransport();
const store = createStore(transport);
const preferences = readPreferences(toast);
let secret = takePairingSecret();
/** @type {import('./types.js').Context} */
const context = {store,transport,preferences,toast,confirm:createConfirm(feedback),
  command:createCommand(store,transport),
  navigate: route => { location.hash = route; },
  setPreference: (name,value) => {
    try { savePreference(preferences,name,value); }
    catch (error) { toast(`Preference could not be saved: ${errorText(error)}`); throw error; }
  },
};
const shell = createShell(host, context);
syncNotifications(context);
const SCREENS = new Set(['pair','fleet','attention','activity','settings','new','agent','controls']);
let cleanup = () => {};
let route = '';

function renderRoute() {
  const incomingSecret = takePairingSecret();
  if (incomingSecret) { secret = incomingSecret; route = ''; }
  // An address this app does not know opens the fleet instead of a dead end.
  const hash = location.hash.slice(1);
  const known = SCREENS.has(hash.split(/[/?]/)[0]);
  if (hash && !known) history.replaceState(null,'',`${location.pathname}${location.search}#fleet`);
  const requested = known ? hash : 'fleet';
  const state = store.get();
  const next = secret || state.connection === 'unpaired' ? 'pair' : requested;
  if (next === route) return;
  const screenName = next.split(/[/?]/)[0];
  if (['new','controls'].includes(screenName) && ['offline','connecting','reconnecting','online'].includes(state.connection) && !state.ready && !state.error) {
    cleanup(); cleanup = () => {}; route = '';
    shell.main.replaceChildren(el('p','notice','Connecting to your workspace…'));
    return;
  }
  cleanup(); shell.main.replaceChildren(); route = next;
  const [path] = next.split('?'); const [screen,id] = path.split('/');
  for (const link of shell.nav.querySelectorAll('a')) {
    if (link.dataset.route === screen) link.setAttribute('aria-current','page');
    else link.removeAttribute('aria-current');
  }
  try { cleanup = mountScreen(screen,id); }
  catch (error) { shell.main.append(el('p','error-banner',errorText(error))); cleanup = () => {}; }
  shell.screen(screen);
  toast.restore();
  document.title = `${screen === 'agent' ? store.get().agents.find(a => a.id === id)?.name || 'Conversation' : screen.charAt(0).toUpperCase()+screen.slice(1)} · Aether`;
  shell.main.focus({preventScroll:true}); shell.main.scrollTop=0; window.scrollTo(0,0);
}

/** @param {string} screen @param {string|undefined} id */
function mountScreen(screen,id) {
  switch (screen) {
    case 'pair': { const pairing = secret; secret = ''; return mountPair(shell.main,context,pairing); }
    case 'fleet': return mountFleet(shell.main,context);
    case 'attention': return mountAttention(shell.main,context);
    case 'activity': return mountActivity(shell.main,context);
    case 'settings': return mountSettings(shell.main,context);
    case 'new': return mountNew(shell.main,context);
    case 'agent': invariant(id, 'Agent id is required'); return mountConversation(shell.main,context,id);
    case 'controls': invariant(id, 'Agent id is required'); return mountControls(shell.main,context,id);
    default: throw new Error(`Unknown screen: ${screen}`);
  }
}

window.addEventListener('hashchange',renderRoute);
store.subscribe(renderRoute); renderRoute();
connect();
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.register('./sw.js',{type:'module'}).catch(error => toast(`Offline setup failed: ${errorText(error)}`));
}
window.addEventListener('online', connect);

function connect() {
  void transport.connect().catch(error => {
    // The transport owns retry state; retain its diagnostic without a competing toast.
    if (store.get().connection !== 'online') store.patch({connectionDetail:errorText(error)});
  });
}
