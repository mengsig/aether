import test from 'node:test';
import assert from 'node:assert/strict';
/** @typedef {{request:Request,respondWith:(response:Promise<Response>)=>void}} FetchEventStub */

async function workerHarness() {
  const names = ['self','caches','fetch'];
  const original = names.map(name => [name,Object.getOwnPropertyDescriptor(globalThis,name)]);
  /** @type {Map<string,(event:FetchEventStub)=>void>} */
  const handlers = new Map();
  /** @type {unknown[]} */ const reads = [];
  const worker = {registration:{scope:'https://aether.test/'},location:{origin:'https://aether.test'},
    addEventListener:(/** @type {string} */ name,/** @type {(event:FetchEventStub)=>void} */ handler) => handlers.set(name,handler)};
  Object.defineProperty(globalThis,'self',{configurable:true,value:worker});
  Object.defineProperty(globalThis,'caches',{configurable:true,value:{open:async () => ({
    match:async (/** @type {unknown} */ request) => { reads.push(request); return new Response('Cached Aether',{status:200}); },
  })}});
  Object.defineProperty(globalThis,'fetch',{configurable:true,value:async () => new Response('Bad Gateway',{status:502})});
  await import('../sw.js');
  const handle = handlers.get('fetch');
  assert.ok(handle, 'The service worker must register a fetch handler');
  return {reads,handle,restore:() => {
    for (const [name,descriptor] of original) {
      if (descriptor) Object.defineProperty(globalThis,/** @type {string} */(name),/** @type {PropertyDescriptor} */(descriptor));
      else Reflect.deleteProperty(globalThis,/** @type {string} */(name));
    }
  }};
}

test('gateway failures recover the cached shell while API requests stay uncached',async () => {
  const harness = await workerHarness();
  try {
    const request = new Request('https://aether.test/');
    Object.defineProperty(request,'mode',{value:'navigate'});
    /** @type {Promise<Response>|undefined} */ let pending;
    harness.handle({request,respondWith:response => { pending = response; }});
    assert.ok(pending);
    const response = await pending;
    assert.equal(response.status,200);
    assert.equal(await response.text(),'Cached Aether');
    assert.deepEqual(harness.reads,['./index.html']);
    harness.handle({request:new Request('https://aether.test/api/pair/status'),respondWith:() => assert.fail('API responses must not be cached')});
  } finally { harness.restore(); }
});
