import { string } from './validate.js';
/** @typedef {{daemonId:string,deviceId:string}} Scope */
/** @typedef {{id:string,name:string,exclusive:boolean}} SubmissionLock */
/** @typedef {{lease?:SubmissionLock,signal?:AbortSignal,ifAvailable?:boolean,waitTimeout?:number}} LockOptions */
export const SUBMISSION_LOCK_WAIT = 3000;
export class SubmissionBusy extends Error {
  constructor() { super('Another tab is still working on this delivery. Try again shortly.'); }
}
let unavailable = false;
/** @type {WeakSet<SubmissionLock>} */
const active = new WeakSet();
/** @param {Scope} scope @param {string} id */
export function submissionLockName(scope,id) {
  string(id,'submission identity');
  if (typeof scope.daemonId !== 'string' || typeof scope.deviceId !== 'string') throw new Error('Invalid submission scope');
  return `aether.remote.submission.v1:${JSON.stringify([scope.daemonId,scope.deviceId,id])}`;
}
/** @param {unknown} lease @param {Scope} scope @param {string} id @returns {asserts lease is SubmissionLock} */
export function assertSubmissionLock(lease,scope,id) {
  if (!lease || typeof lease !== 'object' || !active.has(/** @type {SubmissionLock} */ (lease))) throw new Error('Submission lock lease is no longer active');
  if (/** @type {SubmissionLock} */ (lease).name !== submissionLockName(scope,id)) throw new Error('Submission lock belongs to another receipt or device');
}
/** @template T @param {Scope} scope @param {string} id @param {(lease:SubmissionLock)=>T} action @param {LockOptions} [options] @returns {T|Promise<T>} */
export function withSubmissionLock(scope,id,action,options = {}) {
  const name = submissionLockName(scope,id);
  if (options.lease) {
    assertSubmissionLock(options.lease,scope,id);
    return action(options.lease);
  }
  options.signal?.throwIfAborted();
  const locks = globalThis.navigator?.locks;
  if (unavailable || !locks || typeof locks.request !== 'function') return hold({id,name,exclusive:false},action);
  return requestLock(locks,{id,name,exclusive:true},action,options);
}
/** @template T @param {LockManager} locks @param {SubmissionLock} lease @param {(lease:SubmissionLock)=>T} action @param {LockOptions} options */
async function requestLock(locks,lease,action,options) {
  const waiting = lockWait(options.waitTimeout,options.signal); let entered = false;
  try {
    return await locks.request(lease.name,{mode:'exclusive',...(options.ifAvailable ? {ifAvailable:true} : {signal:waiting.signal})},lock=>{
      entered = true; waiting.cleanup(); waiting.signal.throwIfAborted();
      if (!lock) throw new SubmissionBusy();
      if (lock.name !== lease.name || lock.mode !== 'exclusive') throw new Error('Exclusive submission lock was not granted');
      return hold(lease,action);
    });
  } catch (error) {
    if (entered) throw error;
    if (waiting.signal.aborted) throw waiting.signal.reason;
    unavailable = true; waiting.cleanup();
    return hold({...lease,exclusive:false},action);
  } finally { waiting.cleanup(); }
}
/** @param {number|undefined} timeout @param {AbortSignal|undefined} external */
function lockWait(timeout,external) {
  if (timeout !== undefined && (!Number.isSafeInteger(timeout) || timeout < 1)) throw new Error('Invalid submission lock timeout');
  if (external?.aborted) throw external.reason;
  const controller = new AbortController();
  const abort = () => controller.abort(external?.reason);
  const timer = timeout === undefined ? undefined : setTimeout(()=>controller.abort(new SubmissionBusy()),timeout);
  external?.addEventListener('abort',abort,{once:true});
  return {signal:controller.signal,cleanup:()=>{ clearTimeout(timer); external?.removeEventListener('abort',abort); }};
}
/** @template T @param {SubmissionLock} lease @param {(lease:SubmissionLock)=>T} action @returns {T} */
function hold(lease,action) {
  Object.freeze(lease); active.add(lease);
  try {
    const result = action(lease);
    if (result instanceof Promise) return /** @type {T} */ (result.finally(()=>active.delete(lease)));
    active.delete(lease); return result;
  } catch (error) { active.delete(lease); throw error; }
}
