import { assertAllowed, isMutation } from './contract.js';
import { errorText, string } from './validate.js';
/** @param {import('../types.js').Store} store @param {import('../types.js').Transport} transport */
export function createCommand(store, transport) {
  /** @param {import('../types.js').Command} input */
  return async function command(input) {
    string(input.cmd, 'command'); assertAllowed(input,store.get().role);
    if (store.get().connection !== 'online') throw new Error('Reconnect before sending a command');
    const reply = await transport.request(input);
    try { if (isMutation(input)) await store.refresh(); }
    catch (error) { store.patch({error:`Command accepted; refresh failed: ${errorText(error)}`}); }
    return reply;
  };
}
