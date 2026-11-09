import assert from "node:assert/strict";
import { app, BrowserWindow } from "electron";
import { registerAppScheme, startDesktop, type Desktop } from "../../../src/main/start.ts";
import { DeepLinkRouter } from "../../../src/native/deep-links.ts";

registerAppScheme();
app.whenReady().then(async () => {
  let desktop: Desktop;
  let recovery: Promise<BrowserWindow> | undefined;
  const links = new DeepLinkRouter(() => { recovery = desktop.ensureMainWindow(); });
  desktop = await startDesktop({ offscreen: true, env: process.env, deepLinks: links });
  const first = desktop.appWindow;
  const firstId = first.webContents.id;
  const closedOwners: number[] = [];
  const forgottenOwners: number[] = [];
  const closeOwner = desktop.bridge.connections.closeOwner.bind(desktop.bridge.connections);
  const forget = desktop.bridge.memoryExports.forget.bind(desktop.bridge.memoryExports);
  desktop.bridge.connections.closeOwner = (owner) => { closedOwners.push(owner); closeOwner(owner); };
  desktop.bridge.memoryExports.forget = (owner) => { forgottenOwners.push(owner); forget(owner); };
  const secondary = new BrowserWindow({ show: false, webPreferences: { offscreen: true } });
  const secondaryId = secondary.webContents.id;
  first.destroy();
  assert.equal(secondary.isDestroyed(), false);
  assert.equal(desktop.appWindowIds.has(firstId), false);
  assert.deepEqual(closedOwners, [firstId]);
  assert.deepEqual(forgottenOwners, [firstId]);
  assert.throws(() => desktop.appWindow, /no longer available/);
  assert.equal(links.acceptLink("aether-app://open?route=/fleet"), true);
  assert.equal(links.acceptLink("aether-app://open?route=/inbox"), true);
  assert.equal(links.acceptLink("aether-app://open?route=/invalid"), false);
  const [restored, concurrent] = await Promise.all([recovery, desktop.ensureMainWindow(), desktop.ensureMainWindow()]);
  assert.ok(restored);
  assert.equal(restored, concurrent);
  assert.equal(restored, desktop.appWindow);
  assert.notEqual(restored.webContents.id, firstId);
  assert.deepEqual([...desktop.appWindowIds], [restored.webContents.id]);
  for (let attempt = 0; attempt < 50; attempt++) {
    if (await restored.webContents.executeJavaScript("location.hash") === "#/inbox") break;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.equal(await restored.webContents.executeJavaScript("location.hash"), "#/inbox");
  restored.destroy();
  const original = BrowserWindow.prototype.loadURL;
  BrowserWindow.prototype.loadURL = async () => { throw new Error("injected load failure"); };
  try {
    const failed = desktop.ensureMainWindow();
    await assert.rejects(failed, /injected load failure/);
    assert.equal(desktop.appWindowIds.size, 0);
    assert.equal(secondary.isDestroyed(), false);
  } finally {
    BrowserWindow.prototype.loadURL = original;
  }
  const retry = await desktop.ensureMainWindow();
  assert.equal(desktop.appWindow, retry);
  assert.equal(desktop.appWindowIds.size, 1);
  retry.destroy();
  secondary.destroy();
  assert.equal(desktop.appWindowIds.size, 0);
  assert.deepEqual(closedOwners, forgottenOwners);
  assert.ok(!closedOwners.includes(secondaryId));
  console.log("WINDOW-RECOVERY-OK");
  app.exit(0);
}).catch((error: unknown) => { console.error(error); app.exit(1); });
