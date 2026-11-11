import { el, heading, invariant } from '../../ui/dom.js';
import { listDevices } from '../../core/contract.js';
import { panel, card, form, action, details, replaceRows } from './ui.js';
import { assertOperator } from './protocol.js';
import { notificationSettings } from './notifications.js';
import { deliveryReview } from './outbox.js';
import { dateLabel, words } from './format.js';
import { rawDetails } from './presentation.js';

/** @typedef {import('./ui.js').Panel} Panel */

/** @param {Panel} view */
function preferences(view) {
  const root = card('Appearance', 'Saved on this device. System follows your operating system preferences.');
  root.append(form(view, 'Save appearance', [
    { label: 'Theme', name: 'theme', options: ['system', 'light', 'dark'], value: view.context.preferences.theme },
    { label: 'Motion', name: 'motion', options: ['system', 'reduced', 'full'], value: view.context.preferences.motion },
  ], async values => {
    invariant(['system', 'light', 'dark'].includes(values.theme), 'Choose a supported theme.');
    invariant(['system', 'reduced', 'full'].includes(values.motion), 'Choose a supported motion preference.');
    view.context.setPreference('theme', values.theme);
    view.context.setPreference('motion', values.motion);
    view.status.textContent = 'Appearance saved on this device.';
  }, false));
  return root;
}

/** @param {Panel} view */
function devices(view) {
  const root = card('Paired devices', 'Operators can view paired devices. Changing or revoking access requires the Aether host.');
  const list = el('div', 'stack');
  const refresh = async () => {
    assertOperator(view.context);
    const devices = await listDevices(view.context.transport);
    if (!view.alive()) return;
    assertOperator(view.context);
    replaceRows(list, devices.map(device => {
      const row = card(device.name, device.current ? 'This device' : words(device.role));
      row.append(details({ Access: device.revoked ? 'Revoked' : words(device.role), Paired: dateLabel(device.createdAt),
        'Last seen': dateLabel(device.lastSeenAt) }), rawDetails(device, 'Inspect device details'));
      return row;
    }), 'No paired devices reported.');
  };
  const unsubscribe = view.context.store.subscribe(() => {
    if (view.context.store.get().role !== 'operator') list.replaceChildren();
  });
  root.append(action(view, 'Load paired devices', refresh, true), list,
    el('p', 'muted', 'Use Aether on the host to change or revoke paired devices. Observer devices cannot list other devices. Your current device and connection details appear below.'));
  const commands = el('details', 'controls-record');
  commands.append(el('summary', '', 'Host device instructions'), el('pre', 'controls-preview', 'aether remote devices\naether remote revoke DEVICE'));
  root.append(commands);
  root.append(el('span', 'badge', `Current device: ${view.context.store.get().deviceName || 'Not paired'}`));
  return { root, unsubscribe };
}

/** @param {Panel} view */
function connection(view) {
  const root = card('Connection and recovery');
  const status = el('div');
  const update = () => {
    const state = view.context.store.get();
    const expanded = status.querySelector('details')?.open === true;
    status.replaceChildren(details({ Device: state.deviceName || 'Not paired', Access: words(state.role),
      Connection: state.connectionDetail || words(state.connection) }), rawDetails({ device: state.deviceName,
      role: state.role, connection: state.connection, detail: state.connectionDetail, daemon: state.daemonId,
      version: state.version, last_sequence: state.lastSeq, last_error: state.error }, 'Inspect connection details'));
    const disclosure = status.querySelector('details');
    if (disclosure) disclosure.open = expanded;
    if (state.error) status.append(el('p', 'controls-error', state.error));
  };
  update();
  const unsubscribe = view.context.store.subscribe(update);
  root.append(status, action(view, 'Reconnect', async () => { await view.context.transport.connect(); }),
    action(view, 'Refresh daemon state', async () => { await view.context.store.refresh(); }));
  root.append(el('p', 'muted', 'If access is revoked or the daemon identity changes, verify the daemon before pairing again. Forgetting removes local credentials; it does not revoke a device on the daemon.'));
  root.append(action(view, 'Forget this device', async () => {
    if (!await view.context.confirm('Forget this device', 'Remove the saved identity and local pairing? You will need a fresh pairing link to reconnect.')) return;
    await view.context.transport.forget();
    view.context.navigate('pair');
  }, false, true));
  return { root, unsubscribe };
}

/** @param {HTMLElement} container @param {import('../../types.js').Context} context */
export function mountSettings(container, context) {
  const view = panel(container, context);
  view.body.classList.add('controls-screen');
  const recovery = connection(view);
  const paired = devices(view);
  const deliveries = deliveryReview(context);
  view.body.append(heading('Settings', 'Device preferences, notifications and connection health.'), preferences(view),
    notificationSettings(view), paired.root, deliveries.root, recovery.root,
    card('About Aether', 'Remote app 1.0.1. Your agents continue working when you close this app.'));
  view.sync();
  const requested=new URLSearchParams(location.hash.split('?')[1] || '').get('panel');
  const focus=requested==='outbox' ? requestAnimationFrame(()=>{
    if (!view.alive() || !deliveries.root.isConnected) return;
    deliveries.root.tabIndex=-1;deliveries.root.focus({preventScroll:true});deliveries.root.scrollIntoView({block:'start'});
  }) : 0;
  return () => { cancelAnimationFrame(focus); recovery.unsubscribe(); paired.unsubscribe(); deliveries.destroy(); view.dispose(); };
}
