// Every BrowserWindow option of the app and remote windows, the navigation and window-open
// guards, and the permission handlers, checked against fakes of Electron's objects.

import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { test } from "node:test";
import type { Session, WebContents } from "electron";
import { appWindowOptions } from "../../src/main/app-window.ts";
import { PRELOAD } from "../../src/main/paths.ts";
import { remoteWindowOptions } from "../../src/remote/remote-window.ts";
import { APP_ORIGIN } from "../../src/security/csp.ts";
import { guardSession, pinContents } from "../../src/security/navigation.ts";
import { remotePartition } from "../../src/security/origins.ts";

const HARDENED = {
  contextIsolation: true, sandbox: true, nodeIntegration: false, nodeIntegrationInWorker: false,
  nodeIntegrationInSubFrames: false, webSecurity: true, allowRunningInsecureContent: false, webviewTag: false,
  navigateOnDragDrop: false, safeDialogs: true, spellcheck: false,
};

test("the app window has exactly the hardened options and the one preload", () => {
  assert.deepEqual(appWindowOptions(PRELOAD), {
    title: "Aether", width: 1280, height: 820, minWidth: 720, minHeight: 480, show: false, backgroundColor: "#0a1311",
    webPreferences: { preload: PRELOAD, ...HARDENED },
  });
  assert.throws(() => appWindowOptions("/tmp/other.js"));
});

test("a remote window has no preload and its own per-origin partition", () => {
  const origin = "https://box.example.ts.net";
  const options = remoteWindowOptions(origin);
  assert.deepEqual(options, {
    title: origin, width: 1280, height: 820, minWidth: 480, minHeight: 480, show: false,
    webPreferences: { partition: remotePartition(origin), ...HARDENED },
  });
  assert.equal(options.webPreferences && "preload" in options.webPreferences, false);
  assert.notEqual(remotePartition(origin), remotePartition("https://other.example"));
  assert.ok(remotePartition(origin).startsWith("persist:remote:"));
  assert.throws(() => remoteWindowOptions("https://BOX.example.ts.net"), /normalized/);
});

class FakeContents extends EventEmitter {
  openHandler: ((details: { url: string }) => { action: string }) | undefined;
  setWindowOpenHandler(handler: (details: { url: string }) => { action: string }): void {
    this.openHandler = handler;
  }
  /** Emits `event` for `url`; true when a guard prevented it. */
  attempt(event: string, url: string): boolean {
    let prevented = false;
    this.emit(event, { url, preventDefault: () => { prevented = true; } });
    return prevented;
  }
}

test("navigation and redirects are pinned to the exact origin, popups and webviews denied", () => {
  for (const origin of [APP_ORIGIN, "https://box.example.ts.net"]) {
    const contents = new FakeContents();
    pinContents(contents as unknown as WebContents, origin);
    for (const event of ["will-navigate", "will-frame-navigate", "will-redirect"]) {
      assert.equal(contents.attempt(event, `${origin}/next#x`), false, `${event} within ${origin}`);
      for (const away of ["https://evil.example/", `${origin}.evil.example/`, "http://box.example.ts.net/",
        "https://box.example.ts.net:8443/", "file:///etc/passwd", "app://other/", "javascript:alert(1)", "data:text/html,x"]) {
        if (away.startsWith(origin + "/")) continue;
        assert.equal(contents.attempt(event, away), true, `${event} to ${away} from ${origin}`);
      }
    }
    assert.equal(contents.attempt("will-attach-webview", `${origin}/`), true);
    assert.deepEqual(contents.openHandler?.({ url: `${origin}/` }), { action: "deny" });
  }
  assert.throws(() => pinContents(new FakeContents() as unknown as WebContents, "https://a.example/path"));
});

test("a guarded session allows notifications and nothing else, and never spellchecks", () => {
  let request: ((contents: unknown, permission: string, callback: (granted: boolean) => void) => void) | undefined;
  let check: ((contents: unknown, permission: string) => boolean) | undefined;
  let spellchecker: boolean | undefined;
  let dictionaries: string[] | undefined;
  const fake = {
    setSpellCheckerEnabled: (enabled: boolean) => { spellchecker = enabled; },
    setSpellCheckerLanguages: (languages: string[]) => { dictionaries = languages; },
    setPermissionRequestHandler: (handler: typeof request) => { request = handler; },
    setPermissionCheckHandler: (handler: typeof check) => { check = handler; },
  };
  guardSession(fake as unknown as Session);
  assert.ok(request !== undefined && check !== undefined);
  assert.equal(spellchecker, false, "no session spellchecks");
  assert.deepEqual(dictionaries, [], "no session loads, and so downloads, a dictionary");
  for (const permission of ["geolocation", "media", "clipboard-read", "openExternal", "fullscreen", "hid", "usb", "serial"]) {
    let granted: boolean | undefined;
    request(null, permission, (value) => { granted = value; });
    assert.equal(granted, false, permission);
    assert.equal(check(null, permission), false, permission);
  }
  let notifications: boolean | undefined;
  request(null, "notifications", (value) => { notifications = value; });
  assert.equal(notifications, true);
  assert.equal(check(null, "notifications"), true);
});
