import {createTransport} from './core/transport.js';
import {loadIdentity} from './core/identity.js';
import {pushStatus,registerPush} from './core/contract.js';

/** @param {ServiceWorkerRegistration} registration */
export async function renewSubscription(registration) {
  const identity = await loadIdentity();
  if (!identity?.deviceId) return;
  const transport = createTransport();
  try {
    await transport.connect();
    const config = await pushStatus(transport);
    if (!config.enabled || !config.publicKey) throw new Error('Daemon push is unavailable');
    const existing = await registration.pushManager.getSubscription();
    const key = decodeApplicationKey(config.publicKey);
    const subscription = existing || await registration.pushManager.subscribe({userVisibleOnly:true,applicationServerKey:key});
    const result = await registerPush(transport,subscription.toJSON());
    if (result.reply !== 'push_config') throw new Error('Daemon did not confirm notification renewal');
  } finally { transport.disconnect(); }
}

/** @param {string} encoded */
function decodeApplicationKey(encoded) {
  if (!/^[A-Za-z0-9_-]+$/.test(encoded)) throw new Error('Invalid push public key encoding');
  const key = Uint8Array.from(atob(encoded.replaceAll('-','+').replaceAll('_','/')),c => c.charCodeAt(0));
  if (key.length !== 65 || key[0] !== 4) throw new Error('Invalid push public key');
  return key;
}
