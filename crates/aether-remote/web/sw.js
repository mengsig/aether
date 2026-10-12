/// <reference lib="webworker" />
import {PRECACHE,VERSION} from './precache.js';
import {loadIdentity} from './core/identity.js';
import {notificationData,safeNotificationRoute} from './ui/push.js';
import {renewSubscription} from './push-renewal.js';
const worker = /** @type {ServiceWorkerGlobalScope} */ (/** @type {unknown} */ (self));
const CACHE = `aether-shell-${VERSION}`;
const assetURLs = new Set(PRECACHE.map(path => new URL(path,worker.registration.scope).href));

worker.addEventListener('install',event => {
  event.waitUntil(caches.open(CACHE).then(cache => cache.addAll(PRECACHE)).then(() => worker.skipWaiting()));
});
worker.addEventListener('activate',event => {
  event.waitUntil(caches.keys().then(names => Promise.all(names.filter(name => name.startsWith('aether-shell-') && name !== CACHE).map(name => caches.delete(name)))).then(() => worker.clients.claim()));
});
worker.addEventListener('fetch',event => {
  const request = event.request; const url = new URL(request.url);
  if (request.method !== 'GET' || url.origin !== worker.location.origin || url.pathname.startsWith('/api/') || url.pathname === '/ws') return;
  if (request.mode === 'navigate' || assetURLs.has(url.href)) event.respondWith(shellResponse(request));
});

/** @param {Request} request */
async function shellResponse(request) {
  const cache = await caches.open(CACHE);
  try {
    const response = await fetch(request);
    if (!response.ok) return await cache.match(request.mode === 'navigate' ? './index.html' : request) || response;
    if (response.ok && assetURLs.has(request.url)) await cache.put(request,response.clone());
    return response;
  } catch (error) {
    const cached = await cache.match(request.mode === 'navigate' ? './index.html' : request);
    if (cached) return cached;
    throw error;
  }
}

worker.addEventListener('push',event => { event.waitUntil(showPush(event)); });
worker.addEventListener('pushsubscriptionchange',event => {
  const change = /** @type {ExtendableEvent} */ (event);
  change.waitUntil(renewSubscription(worker.registration));
});
/** @param {PushEvent} event */
async function showPush(event) {
  if (!event.data) return;
  const identity = await loadIdentity();
  if (!identity?.daemonId) return;
  const data = notificationData(event.data.json(),identity.daemonId);
  const options = {body:data.body,icon:'./assets/icon-192.png',badge:'./assets/icon-192.png',tag:data.tag,
    data:{url:data.url},actions:[{action:'open',title:'Review in Aether'}]};
  await worker.registration.showNotification(data.title,options);
}
worker.addEventListener('notificationclick',event => {
  event.notification.close(); event.waitUntil(openNotification(event.notification.data?.url));
});
/** @param {unknown} route */
async function openNotification(route) {
  const url = new URL(safeNotificationRoute(route),worker.registration.scope).href;
  const windows = await worker.clients.matchAll({type:'window',includeUncontrolled:true});
  const existing = windows.find(client => new URL(client.url).origin === worker.location.origin);
  if (existing) { await existing.navigate(url); await existing.focus(); return; }
  await worker.clients.openWindow(url);
}
