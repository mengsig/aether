import { array, object, string } from './validate.js';
import { requestAbort, checkAbort } from './abort.js';
import { base64url, signChallenge } from './identity.js';
import { normalizeDeviceName } from './device-name.js';
export { normalizeDeviceName } from './device-name.js';
export { listWorkspaces, listDirectory, searchFiles, gitStatus } from './host.js';
import { assertRemoteFields } from './permissions.js';
/** @typedef {import('../types.js').Transport} Transport */
/** @typedef {import('./identity.js').Identity} Identity */
/** @typedef {{phrase:string,fingerprint:string,state:string}} PairProgress */
export const PROTOCOL_VERSION = 18;
export const KEYED_COMMANDS = new Set(['create_agent','create_task','send_message','reply_to_question','trigger','create_schedule',
  'resolve_approval','restart','cancel','interrupt','pause','resume','archive','update_agent','set_approvals','update_schedule','create_memory','add_approval_rule']);
export const READ_COMMANDS = new Set(['resolve_submission','hello','status','preview_retention','ping','get_agent','list_agents','get_tree','get_github_watcher','get_schedule','list_schedules','schedule_history','get_task','list_tasks','list_approvals','list_approval_rules','subscribe','unsubscribe','get_events','get_transcript','get_inbox','get_snapshot','get_invocation','list_invocations','get_checkpoint','latest_checkpoint','get_artifact','list_artifacts','read_artifact','memories','list_runs','get_run_report','replay','list_models','auth_status','providers','get_attention','list_workspaces','get_push_config','list_devices','complete_path','get_git_summary','validate_workspace']);
const OPERATOR_READ = new Set(['list_devices','complete_path','get_git_summary','validate_workspace']);
const HOST_ONLY = new Set(['shutdown','engine_tool','engine_tools','export_run','import_run','get_remote','configure_remote','start_pairing','get_pairing','confirm_pairing','cancel_pairing','revoke_device','update_device']);
const DEVICE_SELF = new Set(['subscribe_push','unsubscribe_push','test_push']);
/** @param {import('../types.js').Command} command */
export function isMutation(command) {
  if (command.cmd === 'mcp_servers') return command.refresh === true;
  return !READ_COMMANDS.has(command.cmd);
}
/** @param {import('../types.js').Command} command @param {'operator'|'observer'} role */
export function assertAllowed(command, role) {
  if (command.cmd === 'resolve_submission') {
    const original = /** @type {import('../types.js').Command} */ (object(command.command));
    if (!KEYED_COMMANDS.has(string(original.cmd))) throw new Error('Submission lookup requires a keyed mutation');
    string(original.idempotency_key,'submission key'); assertAllowed(original,role);
  }
  if (OPERATOR_READ.has(command.cmd) && role !== 'operator') throw new Error('Observer devices cannot use this operator-only read');
  if (command.cmd === 'list_devices' && command.include_revoked === true) throw new Error('Revoked devices are visible only on the daemon host');
  if (HOST_ONLY.has(command.cmd)) throw new Error('This command is available only on the daemon host');
  const scope = command.cmd === 'add_approval_rule' ? command.scope : command.cmd === 'resolve_approval' && command.remember ? object(command.remember).scope : null;
  if (scope && object(scope).scope === 'everywhere') throw new Error('Global approval rules can only be changed on the daemon host');
  if (isMutation(command) && !DEVICE_SELF.has(command.cmd) && role !== 'operator') throw new Error('Observer devices cannot change daemon state');
  assertRemoteFields(command);
}
/** @param {string} path @param {Record<string,unknown>} data @param {AbortSignal} [signal] */
async function post(path, data, signal) {
  if (!['/api/pair/claim','/api/pair/status'].includes(path)) throw new Error('Invalid pairing API path');
  const request = requestAbort(15_000,signal);
  try {
    checkAbort(request.signal);
    const response = await fetch(path, {method:'POST',credentials:'omit',headers:{'Content-Type':'application/json'},body:JSON.stringify(data),signal:request.signal});
    const result = object(await response.json(), 'pairing response');
    if (!response.ok) throw pairingError(response,result);
    return result;
  } finally { request.cleanup(); }
}
/** @param {Response} response @param {Record<string,unknown>} result */
function pairingError(response,result) {
  const error=object(result.error); const code=string(error.code); const message=string(error.message);
  if (response.status!==429) return Object.assign(new Error(message),{code});
  const header=Number(response.headers.get('Retry-After'));
  const retryAfter=Number.isSafeInteger(header) && header>0 && header<=3600 ? header : 1;
  return Object.assign(new Error(`Too many pairing attempts. Wait ${retryAfter} ${retryAfter===1?'second':'seconds'}, then try again.`),{code,retryAfter});
}
/** @param {Identity} identity @param {string} secret @param {string} name @param {(progress:PairProgress)=>void} [progress] @param {AbortSignal} [signal] @returns {Promise<Identity>} */
export async function pairDevice(identity, secret, name, progress, signal) {
  string(secret, 'pairing code'); name=normalizeDeviceName(name);
  const publicKey = new Uint8Array(await crypto.subtle.exportKey('raw',identity.keys.publicKey));
  const claim = await post('/api/pair/claim',{code:secret,device_name:name,public_key:base64url(publicKey)},signal);
  const pairing = string(claim.pairing); const phrase = string(claim.phrase); const fingerprint = string(claim.fingerprint);
  const digest = new Uint8Array(await crypto.subtle.digest('SHA-256',publicKey));
  const expected = [...digest].map(byte=>byte.toString(16).padStart(2,'0')).join('');
  if (fingerprint !== expected) throw new Error('Pairing fingerprint does not match this browser key');
  const deadline = Math.min(Date.parse(string(claim.expires_at)),Date.now()+300_000);
  if (!Number.isFinite(deadline) || deadline <= Date.now()) throw new Error('Pairing code has expired');
  progress?.({phrase,fingerprint,state:'claimed'});
  while (Date.now() < deadline) {
    await pollDelay(signal);
    const status = await post('/api/pair/status',{pairing},signal); const state = string(status.state);
    progress?.({phrase,fingerprint,state});
    if (state === 'enrolled') {
      const device = object(status.device); const role = device.role;
      if (role !== 'operator' && role !== 'observer') throw new Error('Invalid enrolled role');
      return {...identity,deviceId:string(device.id),name:string(device.name),role};
    }
    if (state !== 'claimed') throw new Error(`Pairing ${state}; open a new code on the host`);
  }
  throw new Error('Pairing expired before host confirmation');
}
/** @param {AbortSignal} [signal] */
function pollDelay(signal) {
  checkAbort(signal);
  return new Promise((resolve,reject) => {
    const abort = () => { clearTimeout(timer); reject(signal?.reason ?? new Error('Pairing cancelled')); };
    const timer = setTimeout(() => { signal?.removeEventListener('abort',abort); resolve(undefined); },1000);
    signal?.addEventListener('abort',abort,{once:true});
  });
}
/** @param {Identity} identity @param {Record<string,unknown>} challenge @param {string} [origin] @param {string} [clientName] @returns {Promise<import('../types.js').Command>} */
export async function authenticate(identity, challenge, origin = location.origin, clientName = 'aether-web') {
  const deviceId = string(identity.deviceId); const instance = string(challenge.instance_id); const nonce = string(challenge.nonce);
  if (identity.daemonId && identity.daemonId !== instance) throw Object.assign(new Error('Daemon identity changed; verify and pair again'),{code:'identity_changed'});
  if (!/^[A-Za-z0-9_-]{43}$/.test(nonce) || origin.includes('\n') || instance.includes('\n') || deviceId.includes('\n')) throw new Error('Invalid authentication challenge');
  const signature = await signChallenge(identity.keys.privateKey,['aether-remote-auth-v1',instance,origin,deviceId,nonce].join('\n'));
  return {cmd:'hello',client_name:clientName,client_version:'1.0.1',protocol_version:PROTOCOL_VERSION,features:['deltas','artifact_bytes'],device:{id:deviceId,signature}};
}
export function socketURL() {
  const url = new URL('/ws',location.href); url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:'; return url.href;
}
/** @param {Transport} transport */
export async function listDevices(transport) {
  const reply = await transport.request({cmd:'list_devices'});
  return array(reply.data).map(value => {
    const device = object(value); const role = device.role;
    if (role !== 'operator' && role !== 'observer') throw new Error('Invalid device role');
    return {id:string(device.id),name:string(device.name),role,revoked:device.revoked_at != null,createdAt:string(device.created_at),lastSeenAt:device.last_seen_at == null ? null : string(device.last_seen_at),current:device.current === true};
  });
}
/** @param {Transport} transport @param {string} id */
export const revokeDevice = (transport,id) => transport.request({cmd:'revoke_device',id});
/** @param {Transport} transport */
export async function pushStatus(transport) {
  const data = object((await transport.request({cmd:'get_push_config'})).data);
  if (typeof data.enabled !== 'boolean') throw new Error('Invalid push capability');
  return {enabled:data.enabled,subscribed:data.subscribed === true,publicKey:typeof data.vapid_public_key === 'string' ? data.vapid_public_key : null};
}
/** @param {Transport} transport @param {unknown} subscription */
export function registerPush(transport,subscription) {
  const value = object(subscription); const keys = object(value.keys);
  return transport.request({cmd:'subscribe_push',subscription:{endpoint:string(value.endpoint),p256dh:string(keys.p256dh),auth:string(keys.auth)},kinds:[]});
}
/** @param {Transport} transport */
export const removePush = transport => transport.request({cmd:'unsubscribe_push'});
