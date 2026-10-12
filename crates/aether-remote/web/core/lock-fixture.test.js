import assert from 'node:assert/strict';

export function fakeLocks() {
  const queues = new Map();
  return {
    request(name, options, action) {
      assert.equal(options.mode, 'exclusive');
      assert.equal(typeof name, 'string');
      return new Promise((resolve, reject) => {
        const queue = queues.get(name) ?? [];
        if (options.ifAvailable && queue.length) {
          Promise.resolve().then(()=>action(null)).then(resolve,reject); return;
        }
        const entry = {action, resolve, reject, running:false, signal:options.signal};
        const abort = () => {
          if (entry.running) return;
          queue.splice(queue.indexOf(entry), 1);
          reject(options.signal.reason);
        };
        entry.abort = abort;
        if (options.signal?.aborted) { reject(options.signal.reason); return; }
        options.signal?.addEventListener('abort', abort, {once:true});
        queue.push(entry); queues.set(name, queue);
        if (queue.length === 1) queueMicrotask(() => run(name, queue));
      });
    },
    pending(name) { return queues.get(name)?.filter(entry => !entry.running).length ?? 0; },
  };
  async function run(name, queue) {
    const entry = queue[0];
    if (!entry) { queues.delete(name); return; }
    entry.running = true;
    entry.signal?.removeEventListener('abort', entry.abort);
    try { entry.resolve(await entry.action({name, mode:'exclusive'})); }
    catch (error) { entry.reject(error); }
    finally { queue.shift(); run(name, queue); }
  }
}
