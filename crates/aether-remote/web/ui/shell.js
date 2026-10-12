import {el, button} from './dom.js';
import {icon} from './icons.js';
import {pageChrome} from './page-chrome.js';
import {checkingDelivery} from './delivery-actions.js';

/** @param {HTMLElement} host @param {import('../types.js').Context} context */
export function createShell(host, context) {
  const shell = el('div', 'app-shell');
  const topbar = el('header', 'topbar');
  const brand = el('a', 'brand'); brand.href = '#fleet';
  brand.append(el('span', 'brand-mark', 'a'), el('span', '', 'aether'));
  const status = el('span', 'connection'); status.setAttribute('role', 'status');
  const topActions = el('div', 'row top-actions');
  const newWork = button('', () => context.navigate('new'), 'button icon-button');
  newWork.setAttribute('aria-label', 'Start new work'); newWork.append(icon('plus'));
  topActions.append(status, newWork); topbar.append(brand, topActions);
  const banner = el('div', 'global-banner'); banner.setAttribute('role', 'status'); banner.hidden = true;
  const main = el('main', 'main-content'); main.id = 'main-content'; main.tabIndex = -1;
  const nav = navigation(); shell.append(topbar, banner, main, nav);
  host.replaceChildren(shell);
  trackNavigationSpace(shell,nav);
  document.querySelector('.skip-link')?.addEventListener('click',event => { event.preventDefault(); main.focus(); });
  const update = () => {
    const state = context.store.get(); status.dataset.state = state.connection; shell.dataset.connection = state.connection;
    const labels = {online:'Connected',unpaired:'Not paired',connecting:'Connecting…',reconnecting:'Reconnecting…',offline:'Offline · Your drafts are safe',revoked:'Access revoked','identity-changed':'Verify this host'};
    status.textContent = labels[state.connection];
    status.title = status.textContent;
    status.setAttribute('aria-label',status.textContent);
    nav.hidden = state.connection === 'unpaired' || shell.dataset.screen === 'agent';
    const badge = nav.querySelector('.badge');
    if (badge instanceof HTMLElement) { badge.textContent = String(state.attention.length); badge.hidden = !state.attention.length; }
    newWork.hidden = state.role === 'observer' || state.connection === 'unpaired';
    const uncertain = state.outbox.filter(item => item.status === 'unknown' && !checkingDelivery(item)).length;
    const error = state.error && state.connection === 'online' ? 'Workspace updates need attention. Open Settings for details.' : '';
    const message = error || connectionMessage(state) || (uncertain ? `${uncertain} deliveries are unconfirmed. Review them in Settings.` : '');
    banner.hidden = !message; banner.textContent = message;
  };
  context.store.subscribe(update); update();
  const page = pageChrome(shell,topbar,main);
  return {main,nav,shell,screen:(/** @type {string} */ name)=>{page(name);update();}};
}

/** @param {HTMLElement} shell @param {HTMLElement} nav */
function trackNavigationSpace(shell,nav) {
  const update=()=>{
    const height=nav.getBoundingClientRect().height;const bottom=Number.parseFloat(getComputedStyle(nav).bottom);
    if (!height || !Number.isFinite(bottom)) return;
    shell.style.setProperty('--navigation-space',`${Math.ceil(height+bottom+12)}px`);
  };
  new ResizeObserver(update).observe(nav);window.addEventListener('resize',update);
}

function navigation() {
  const nav = el('nav', 'bottom-nav'); nav.setAttribute('aria-label', 'Main navigation');
  /** @type {Array<['fleet'|'attention'|'activity'|'settings',string]>} */
  const routes = [['fleet','Fleet'],['attention','Attention'],['activity','Activity'],['settings','Settings']];
  for (const [route, label] of routes) {
    const link = el('a', 'nav-item'); link.href = `#${route}`; link.dataset.route = route;
    link.append(icon(route), el('span', '', label));
    if (route === 'attention') { const badge = el('span', 'badge attention'); badge.hidden = true; link.append(badge); }
    nav.append(link);
  }
  return nav;
}

/** @param {import('../types.js').State} state */
function connectionMessage(state) {
  if (state.connection === 'revoked') return 'This device has been revoked. Pair again from Settings to reconnect.';
  if (state.connection === 'identity-changed') return 'The daemon identity changed. Review the host and pair again in Settings.';
  if (state.connection === 'online' && state.role === 'observer') return 'Observer access · You can follow work. Changes require an operator device.';
  return '';
}
