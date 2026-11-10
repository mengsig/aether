/** @param {AbortSignal} [signal] */
export function checkAbort(signal) {
  if (signal?.aborted) throw signal.reason ?? new Error('Operation cancelled');
}
/** @param {number} timeout @param {AbortSignal} [external] */
export function requestAbort(timeout, external) {
  if (!Number.isSafeInteger(timeout) || timeout <= 0) throw new Error('Invalid request timeout');
  const controller = new AbortController();
  const abort = () => controller.abort(external?.reason ?? new Error('Operation cancelled'));
  const timer = setTimeout(() => controller.abort(new Error('Pairing request timed out')),timeout);
  if (external?.aborted) abort();
  else external?.addEventListener('abort',abort,{once:true});
  return {signal:controller.signal,cleanup:() => { clearTimeout(timer); external?.removeEventListener('abort',abort); }};
}
