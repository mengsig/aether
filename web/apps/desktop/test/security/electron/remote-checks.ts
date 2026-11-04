// Checks on remote windows, run inside Electron against the fixture HTTPS listener: a plain
// browser window with no preload, its own partition, pinned navigation and no IPC.

import assert from "node:assert/strict";
import { session, type IpcMainInvokeEvent, type WebContents } from "electron";
import { senderOf } from "../../../src/main/host.ts";
import { remotePartition } from "../../../src/security/origins.ts";
import { effectivePreferences, type Context } from "./context.ts";

async function appCall(ctx: Context, body: string): Promise<{ ok?: unknown; error?: string }> {
  const contents = ctx.desktop.appWindow.webContents;
  return ctx.js(contents, `(async () => { ${body} })().then((value) => ({ ok: value ?? null }),
    (error) => ({ error: String(error.message ?? error) }))`) as Promise<{ ok?: unknown; error?: string }>;
}

function remoteContents(ctx: Context): WebContents {
  const window = ctx.desktop.remote.window(ctx.server.origin);
  assert.ok(window !== undefined && !window.isDestroyed(), "the remote window is open");
  return window.webContents;
}

export async function openRemoteChecks(ctx: Context): Promise<void> {
  const origin = ctx.server.origin;
  await ctx.check("openRemote refuses http, paths, userinfo and unlisted origins", async () => {
    const port = new URL(origin).port;
    for (const value of [`http://localhost:${port}`, `${origin}/path`, `https://user@localhost:${port}`, origin]) {
      const result = await appCall(ctx, `return desktop.openRemote(${JSON.stringify(value)});`);
      assert.ok(result.error !== undefined, `refused ${value}`);
    }
    assert.equal(ctx.desktop.remote.window(origin), undefined);
    assert.deepEqual(ctx.server.seen, []);
  });
  await ctx.check("openRemote opens a listed origin", async () => {
    const listed = await appCall(ctx, `await desktop.setPreference("remote.origins", [${JSON.stringify(origin)}]);
      return desktop.openRemote(${JSON.stringify(`${origin}/`)});`);
    assert.deepEqual(listed, { ok: null });
    assert.ok(ctx.desktop.remote.window(origin) !== undefined);
  });
}

export async function remoteWindowChecks(ctx: Context): Promise<void> {
  const origin = ctx.server.origin;
  await ctx.check("remote window preferences", async () => {
    const contents = remoteContents(ctx);
    const prefs = effectivePreferences(contents);
    assert.ok(prefs.preload === undefined || prefs.preload === "", "no preload");
    assert.equal(prefs.sandbox, true);
    assert.equal(prefs.contextIsolation, true);
    assert.equal(prefs.webSecurity, true);
    assert.equal(prefs.nodeIntegration, false);
    assert.equal(prefs.webviewTag, false);
  });
  await ctx.check("remote window has its own partition", async () => {
    const contents = remoteContents(ctx);
    assert.equal(contents.session, session.fromPartition(remotePartition(origin)));
    assert.notEqual(contents.session, ctx.desktop.appWindow.webContents.session);
    assert.match(remotePartition(origin), /^persist:remote:[0-9a-f]{64}$/);
  });
  await ctx.check("remote window authenticates with Origin and a SameSite=Strict cookie", async () => {
    await ctx.until("the fixture to see an authenticated request", () => ctx.server.authenticated);
    assert.deepEqual(ctx.server.reports, [
      { desktop: "undefined", require: "undefined", process: "undefined" },
      { login: 204, me: 200 },
    ]);
    const cookies = await remoteContents(ctx).session.cookies.get({ name: "session" });
    assert.equal(cookies.length, 1);
    assert.equal(cookies[0]?.sameSite, "strict");
  });
  await ctx.check("remote window keeps its origin as its title", async () => {
    assert.equal(ctx.desktop.remote.window(origin)?.getTitle(), origin);
  });
  await ctx.check("app window has no request in the remote session", async () => {
    const appCookies = await ctx.desktop.appWindow.webContents.session.cookies.get({});
    assert.deepEqual(appCookies, []);
  });
}

export async function remoteIsolationChecks(ctx: Context): Promise<void> {
  const origin = ctx.server.origin;
  const other = `https://127.0.0.1:${ctx.server.port}`;
  await ctx.check("remote navigation is pinned to its origin", async () => {
    const contents = remoteContents(ctx);
    await ctx.js(contents, `location.href = ${JSON.stringify(`${other}/landing`)}; true`);
    await ctx.settle();
    assert.equal(contents.getURL(), `${origin}/`);
  });
  await ctx.check("remote redirects are pinned to its origin", async () => {
    const contents = remoteContents(ctx);
    await ctx.js(contents, `location.href = "/redirect"; true`);
    await ctx.until("the fixture to see the redirect", () => ctx.server.seen.some((seen) => seen.path === "/redirect"));
    await ctx.settle();
    assert.ok(!ctx.server.seen.some((seen) => seen.path === "/landing"), "the redirect target was never loaded");
    assert.ok(contents.getURL().startsWith(`${origin}/`), contents.getURL());
  });
  await ctx.check("remote window opens no windows", async () => {
    assert.equal(await ctx.js(remoteContents(ctx), `window.open(${JSON.stringify(`${origin}/`)}) === null`), true);
  });
  await ctx.check("remote window gets no permissions", async () => {
    const code = await ctx.js(remoteContents(ctx), `new Promise((resolve) => navigator.geolocation.getCurrentPosition(
      () => resolve("granted"), (error) => resolve(error.code), { timeout: 5000 }))`);
    assert.equal(code, 1, "PERMISSION_DENIED");
  });
  await ctx.check("no IPC handler answers a remote window", async () => {
    const contents = remoteContents(ctx);
    const event = { sender: contents, senderFrame: contents.mainFrame } as unknown as IpcMainInvokeEvent;
    const sender = senderOf(event, ctx.desktop.appWindowIds);
    assert.equal(sender.isAppWindow, false);
    for (const [method, args] of [["discover", []], ["connect", ["aether:local"]], ["diagnostics", []]] as const) {
      await assert.rejects(ctx.desktop.bridge.handle(sender, contents.id, method, args), /app window/i, method);
    }
  });
}

export async function forgetChecks(ctx: Context): Promise<void> {
  const origin = ctx.server.origin;
  await ctx.check("removing an origin clears its partition", async () => {
    const partition = session.fromPartition(remotePartition(origin));
    assert.ok((await partition.cookies.get({})).length > 0, "the session cookie was stored");
    const result = await appCall(ctx, `return desktop.setPreference("remote.origins", []);`);
    assert.deepEqual(result, { ok: null });
    await ctx.until("the window to close", () => ctx.desktop.remote.window(origin) === undefined);
    assert.deepEqual(await partition.cookies.get({}), []);
  });
}
