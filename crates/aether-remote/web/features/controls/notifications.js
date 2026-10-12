import { el, invariant, errorText } from '../../ui/dom.js';
import { pushStatus, registerPush, removePush } from '../../core/contract.js';
import { card, action, details } from './ui.js';
import { assertDeviceSelf, required, record, replyData } from './protocol.js';

/** @typedef {import('./ui.js').Panel} Panel */

/** @param {string} publicKey */
export function applicationKey(publicKey) {
  required(publicKey, 'Push public key');
  invariant(/^[A-Za-z0-9_\-=]+$/.test(publicKey), 'Invalid push public key encoding.');
  const binary = atob(publicKey.replaceAll('-', '+').replaceAll('_', '/'));
  invariant(binary.length === 65 && binary.charCodeAt(0) === 4, 'Expected an uncompressed P-256 push public key.');
  return Uint8Array.from(binary, char => char.charCodeAt(0));
}

async function registration() {
  invariant('serviceWorker' in navigator, 'This browser does not support service workers.');
  const worker = await navigator.serviceWorker.getRegistration();
  invariant(worker?.active, 'The app service worker is not active. Reload this page and try again.');
  invariant('pushManager' in worker, 'Push notifications are not supported by this browser.');
  return worker;
}

/** @param {Panel} view */
async function enable(view) {
  assertDeviceSelf(view.context);
  invariant('Notification' in globalThis, 'Notifications are unavailable. Install this app if your browser requires it.');
  const permission = await Notification.requestPermission();
  invariant(permission === 'granted', 'Notification permission was not granted. You can change it in browser settings.');
  const capability = await pushStatus(view.context.transport);
  invariant(capability.enabled && capability.publicKey, 'This daemon does not have Web Push configured.');
  const worker = await registration();
  assertDeviceSelf(view.context);
  const existing = await worker.pushManager.getSubscription();
  assertDeviceSelf(view.context);
  const subscription = existing || await worker.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: applicationKey(capability.publicKey) });
  assertDeviceSelf(view.context);
  try {
    const result = record(replyData(await registerPush(view.context.transport, subscription.toJSON()), 'push_config'));
    invariant(result.subscribed === true, 'The daemon did not confirm an active subscription.');
  }
  catch (error) { throw new Error(`Browser subscription exists, but daemon registration failed: ${errorText(error)}. Check this daemon response before enabling again.`, { cause: error }); }
  try { localStorage.removeItem('aether.push.disabled'); }
  catch (error) { throw new Error(`Notifications are subscribed, but the local opt-out preference could not be cleared: ${errorText(error)}`, { cause: error }); }
  view.status.textContent = 'The daemon accepted this browser’s notification subscription.';
}

/** @param {Panel} view */
async function disable(view) {
  assertDeviceSelf(view.context);
  const result = record(replyData(await removePush(view.context.transport), 'push_config'));
  invariant(result.subscribed === false, 'The daemon did not confirm subscription removal.');
  const cleanup = await Promise.allSettled([
    Promise.resolve().then(() => localStorage.setItem('aether.push.disabled', 'true')),
    clearBrowserSubscription(),
  ]);
  const failures = cleanup.filter(result => result.status === 'rejected').map(result => result.reason);
  if (failures.length) throw new AggregateError(failures, `Daemon subscription removed, but local cleanup failed: ${failures.map(errorText).join('; ')}`);
  view.status.textContent = 'Notification subscription removed.';
}

async function clearBrowserSubscription() {
  if (!('serviceWorker' in navigator)) return;
  const worker = await navigator.serviceWorker.getRegistration();
  if (!worker || !('pushManager' in worker)) return;
  const subscription = await worker.pushManager.getSubscription();
  if (subscription) invariant(await subscription.unsubscribe(), 'Browser unsubscribe was not confirmed.');
}

/** @param {Panel} view */
export function notificationSettings(view) {
  const root = card('Notifications', 'Receive attention updates while the app is closed. Browser permission and a configured push service are both required.');
  const status = el('div');
  const refresh = async () => {
    const capability = await pushStatus(view.context.transport);
    if (!view.alive()) return;
    status.replaceChildren(details({ permission: 'Notification' in globalThis ? Notification.permission : 'unsupported',
      daemon_push: capability.enabled ? 'configured' : 'not configured',
      this_device: capability.subscribed ? 'subscribed' : 'not subscribed' }));
  };
  status.append(el('p', 'muted', `Browser permission: ${'Notification' in globalThis ? Notification.permission : 'unsupported'}`));
  root.append(status, action(view, 'Check notification support', refresh));
  const actions = el('div', 'row controls-actions');
  actions.append(action(view, 'Enable notifications', async () => { await enable(view); await refresh(); }, 'device'),
    action(view, 'Disable notifications', async () => { await disable(view); await refresh(); }, 'device'));
  actions.append(action(view, 'Send test notification', () => testNotification(view,refresh), 'device'));
  root.append(actions);
  return root;
}

/** @param {Panel} view @param {()=>Promise<void>} refresh */
async function testNotification(view,refresh) {
  assertDeviceSelf(view.context);
  try {
    const reply = await view.context.command({cmd:'test_push'});
    invariant(reply.reply === 'ok','The push service did not confirm the test notification.');
  } catch (error) {
    try { await refresh(); }
    catch (refreshError) { throw new AggregateError([error,refreshError],`Test notification failed: ${errorText(error)}. Subscription status could not be refreshed: ${errorText(refreshError)}`); }
    throw new Error(`Test notification was not delivered: ${errorText(error)}. If its subscription expired, disable then enable notifications to renew it.`,{cause:error});
  }
  await refresh();
  view.status.textContent = 'The push service accepted the test notification.';
}
