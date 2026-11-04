// The guards every window gets (ARCHITECTURE.md §c): navigation and redirects pinned to one
// origin, window.open denied, no <webview>, and permissions denied except notifications.

import type { Session, WebContents } from "electron";

/** The one permission a window may hold. */
export function allowOnlyNotifications(permission: string): boolean {
  return permission === "notifications";
}

/** The exact origin of `url`, or undefined for an opaque or unparsable one. */
export function originOf(url: string): string | undefined {
  const parsed = URL.parse(url);
  if (parsed === null) return undefined;
  if (parsed.protocol === "app:") return `app://${parsed.host}`;
  return parsed.origin === "null" ? undefined : parsed.origin;
}

/** Pins every frame's navigation and every redirect to `origin`; refuses popups and webviews. */
export function pinContents(contents: WebContents, origin: string): void {
  if (originOf(`${origin}/`) !== origin) throw new Error(`${origin} is an exact origin`);
  const pin = (event: { url: string; preventDefault(): void }) => {
    if (originOf(event.url) !== origin) event.preventDefault();
  };
  contents.on("will-frame-navigate", pin);
  contents.on("will-navigate", pin);
  contents.on("will-redirect", pin);
  contents.on("will-attach-webview", (event) => event.preventDefault());
  contents.setWindowOpenHandler(() => ({ action: "deny" }));
}

/** Deny-by-default permission handlers on `session`, notifications excepted. */
export function guardSession(session: Session): void {
  session.setPermissionRequestHandler((_contents, permission, callback) => callback(allowOnlyNotifications(permission)));
  session.setPermissionCheckHandler((_contents, permission) => allowOnlyNotifications(permission));
}
