// The browser half of push: subscribing through the service worker with the daemon's VAPID key.
// The subscription it returns is what `subscribe_push` carries; nothing is kept here.

import type { aether } from "@aether-ui/protocol";
import { invariant } from "../kit/invariant.ts";

/** Whether this browser can hold a push subscription at all. */
export function pushSupported(): boolean {
  return typeof navigator !== "undefined" && "serviceWorker" in navigator;
}

/** The VAPID public key (base64url, unpadded) as the bytes `pushManager.subscribe` wants. */
export function vapidKeyBytes(key: string): Uint8Array<ArrayBuffer> {
  invariant(/^[A-Za-z0-9_-]+$/.test(key), "a VAPID key is base64url");
  const padded = key.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(key.length / 4) * 4, "=");
  const raw = atob(padded);
  const bytes = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i += 1) bytes[i] = raw.charCodeAt(i);
  invariant(bytes.length === 65, "a VAPID key is an uncompressed P-256 point");
  return bytes;
}

/** Subscribes this browser and returns the subscription in the daemon's shape. */
export async function subscribeBrowser(vapidPublicKey: string): Promise<aether.PushSubscription> {
  invariant(pushSupported(), "push is supported before subscribing");
  const registration = await navigator.serviceWorker.ready;
  const subscription = await registration.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: vapidKeyBytes(vapidPublicKey) });
  const json = subscription.toJSON();
  const { endpoint } = json;
  const p256dh = json.keys?.p256dh;
  const auth = json.keys?.auth;
  if (!endpoint || !p256dh || !auth) throw new Error("The browser gave back an incomplete push subscription");
  return { endpoint, p256dh, auth };
}

/** Drops this browser's own subscription, if it has one. */
export async function unsubscribeBrowser(): Promise<void> {
  invariant(pushSupported(), "push is supported before unsubscribing");
  const registration = await navigator.serviceWorker.ready;
  const subscription = await registration.pushManager.getSubscription();
  if (subscription && !(await subscription.unsubscribe())) throw new Error("The browser did not drop its push subscription");
}
