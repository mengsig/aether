import {pushStatus,registerPush} from '../core/contract.js';
import {errorText} from './dom.js';

/** @param {import('../types.js').Context} context */
export function syncNotifications(context) {
  let running = false; let attempted = false;
  const update = () => {
    if (context.store.get().connection !== 'online') { attempted = false; return; }
    if (running || attempted || !('Notification' in globalThis) || Notification.permission !== 'granted') return;
    running = true; attempted = true;
    reconcile(context).catch(error => context.toast(`Notification connection needs attention: ${errorText(error)}`)).finally(() => { running = false; });
  };
  context.store.subscribe(update); update();
}

/** @param {import('../types.js').Context} context */
async function reconcile(context) {
  if (!('serviceWorker' in navigator) || localStorage.getItem('aether.push.disabled') === 'true') return;
  const registration = await navigator.serviceWorker.ready;
  const existing = await registration.pushManager?.getSubscription();
  if (!existing) return;
  const status = await pushStatus(context.transport);
  if (status.enabled && !status.subscribed) await registerPush(context.transport,existing.toJSON());
}
