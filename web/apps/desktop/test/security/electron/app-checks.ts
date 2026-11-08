// Checks on the app window, run inside Electron: its preferences, the same-bundle app://, its
// navigation and CSP, and the bridge as the shared frontend's page reaches it.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { net, session, type IpcMainInvokeEvent } from "electron";
import { buildSync } from "esbuild";
import { APP_CSP } from "../../../src/security/csp.ts";
import { senderOf } from "../../../src/main/host.ts";
import { WEB_DIST } from "../../../src/main/paths.ts";
import { bundleFiles } from "../bundle-files.ts";
import { effectivePreferences, type Context } from "./context.ts";
import { REMOTE_VIEW } from "./fixtures.ts";

const MEMORY = "mem_01JAAAAAAAAAAAAAAAAAAAAAAA";

export async function appWindowChecks(ctx: Context): Promise<void> {
  const contents = ctx.desktop.appWindow.webContents;
  await ctx.check("app window preferences", async () => {
    const prefs = effectivePreferences(contents);
    assert.equal(prefs.contextIsolation, true);
    assert.equal(prefs.sandbox, true);
    assert.equal(prefs.nodeIntegration, false);
    assert.equal(prefs.webviewTag, false);
    assert.equal(prefs.webSecurity, true);
    assert.equal(prefs.nodeIntegrationInSubFrames, false);
    assert.equal(contents.session, session.defaultSession);
  });
  await ctx.check("window.desktop is the only bridge", async () => {
    const seen = await ctx.js(contents, `({ type: typeof window.desktop, frozen: Object.isFrozen(window.desktop),
      keys: Object.keys(window.desktop).sort(), require: typeof require, process: typeof process })`);
    assert.deepEqual(seen, {
      type: "object", frozen: true, require: "undefined", process: "undefined",
      keys: ["close", "connect", "diagnostics", "discover", "getPreference", "notify", "onDeepLink", "openExternal",
        "openRemote", "pickDirectory", "privileged", "request", "setPreference", "setup", "startDaemon", "subscribe", "version"],
    });
  });
  await ctx.check("app:// serves web/dist byte for byte", async () => {
    const files = bundleFiles();
    assert.ok(files.includes("index.html") && files.length > 3, "the bundle has its entry");
    for (const file of files) {
      const response = await net.fetch(`app://aether/${file}`);
      assert.equal(response.status, 200, file);
      assert.equal(response.headers.get("content-security-policy"), APP_CSP, file);
      assert.ok(Buffer.from(await response.arrayBuffer()).equals(readFileSync(path.join(WEB_DIST, file))), file);
    }
    const root = Buffer.from(await (await net.fetch("app://aether/")).arrayBuffer());
    assert.ok(root.equals(readFileSync(path.join(WEB_DIST, "index.html"))), "app://aether/ is index.html");
  });
  await ctx.check("app:// refuses paths outside the bundle", async () => {
    for (const url of ["app://aether/../package.json", "app://aether/%2e%2e/package.json", "app://aether/.hidden",
      "app://aether/assets/%2F..%2F..%2Fpackage.json", "app://other/index.html", "app://aether/missing.js"]) {
      assert.equal((await net.fetch(url)).status, 404, url);
    }
  });
}

export async function appIsolationChecks(ctx: Context): Promise<void> {
  const contents = ctx.desktop.appWindow.webContents;
  await ctx.check("app window navigation is pinned", async () => {
    await ctx.js(contents, `location.href = ${JSON.stringify(`${ctx.server.origin}/`)}; true`);
    await ctx.settle();
    assert.equal(contents.getURL(), "app://aether/index.html");
  });
  await ctx.check("app window opens no windows", async () => {
    assert.equal(await ctx.js(contents, `window.open(${JSON.stringify(`${ctx.server.origin}/`)}) === null`), true);
  });
  await ctx.check("app CSP blocks the remote origin", async () => {
    const outcome = await ctx.js(contents, `fetch(${JSON.stringify(`${ctx.server.origin}/api/me`)}, { method: "POST" })
      .then(() => "reached", () => "blocked")`);
    assert.equal(outcome, "blocked");
    assert.match(APP_CSP, /connect-src 'self';/);
    assert.ok(!APP_CSP.includes("https:") && !APP_CSP.includes(ctx.server.origin));
  });
  await ctx.check("subframes get no bridge", async () => {
    const inFrame = await ctx.js(contents, `(() => {
      const frame = document.createElement("iframe");
      document.body.append(frame);
      return typeof frame.contentWindow.desktop;
    })()`);
    assert.equal(inFrame, "undefined");
    const subframe = contents.mainFrame.frames[0];
    assert.ok(subframe !== undefined, "the page has its subframe");
    const event = { sender: contents, senderFrame: subframe } as unknown as IpcMainInvokeEvent;
    const sender = senderOf(event, ctx.desktop.appWindowIds);
    assert.equal(sender.isAppWindow, true);
    await assert.rejects(ctx.desktop.bridge.handle(sender, contents.id, "discover", []), /subframe/);
  });
  await ctx.check("app window has no seen request at the remote origin", async () => {
    assert.deepEqual(ctx.server.seen, []);
  });
}

export async function bridgeChecks(ctx: Context): Promise<void> {
  const contents = ctx.desktop.appWindow.webContents;
  const call = (body: string) => ctx.js(contents, `(async () => { ${body} })().then((value) => ({ ok: value }),
    (error) => ({ error: String(error.message ?? error) }))`) as Promise<{ ok?: unknown; error?: string }>;
  await ctx.check("bridge passes a status request", async () => {
    const result = await call(`const conn = await desktop.connect("aether:local");
      window.testConn = conn;
      return desktop.request(conn, { cmd: "status", req: 9001 });`);
    assert.deepEqual(result.ok, { frame: "reply", id: 9001, result: { Ok: { reply: "status", data: { ok: true } } } });
    const daemons = await call("return desktop.discover();");
    assert.ok(Array.isArray(daemons.ok) && !JSON.stringify(daemons.ok).includes(".sock"), "no socket path reaches the page");
  });
  await ctx.check("bridge refuses shutdown, host-only and remote-admin commands", async () => {
    for (const body of [
      `return desktop.request(testConn, { cmd: "shutdown", req: 9002 });`,
      `return desktop.subscribe(testConn, { cmd: "shutdown", req: 9003 }, () => {});`,
      `return desktop.privileged({ kind: "shutdown", conn: testConn });`,
      `return desktop.request(testConn, { cmd: "accept_memory", id: "${MEMORY}", req: 9004 });`,
      `return desktop.subscribe(testConn, { cmd: "get_remote", req: 9005 }, () => {});`,
      `return desktop.request(testConn, { cmd: "configure_remote", expected_revision: 7, settings: { enabled: true }, req: 9008 });`,
      `return desktop.request(testConn, { cmd: "start_pairing", role: "operator", req: 9009 });`,
      `return desktop.request(testConn, { cmd: "no_such_command", req: 9006 });`,
      `return desktop.request(testConn, { cmd: "send_message", id: "x", text: "a".repeat(1024 * 1024), req: 9007 });`,
    ]) {
      const result = await call(body);
      const reply = result.ok as { result?: { Err?: { code?: string } } } | undefined;
      assert.ok(result.error !== undefined || reply?.result?.Err?.code === "forbidden", `refused: ${body.slice(0, 80)}`);
    }
    assert.ok(!ctx.daemon.lines.some((line) => /"req":900[2-9]\b/.test(line)), "nothing refused reached the daemon");
  });
  await ctx.check("bridge passes the local remote status read", async () => {
    const result = await call(`return desktop.request(testConn, { cmd: "get_remote", req: 9010 });`);
    assert.deepEqual(result.ok, { frame: "reply", id: 9010, result: { Ok: { reply: "remote", data: REMOTE_VIEW } } });
    assert.ok(ctx.daemon.lines.some((line) => line.includes('"req":9010')));
  });
  await ctx.check("refused requests preserve the renderer transport", async () => {
    const bundle = buildSync({ entryPoints: [new URL("../../../../../packages/core/src/transports/ipc.ts", import.meta.url).pathname],
      bundle: true, write: false, format: "iife", globalName: "transportTest", platform: "browser" }).outputFiles[0];
    assert.ok(bundle !== undefined, "the transport bundle exists");
    const result = await ctx.js(contents, `(async () => {
      ${bundle.text}
      const frames = []; let closed = false;
      const link = await transportTest.ipcTransport(desktop, () => {}).open("aether:local", f => frames.push(f), () => { closed = true; });
      link.send({ cmd: "shutdown", req: 9101 });
      link.send({ cmd: "subscribe", req: 9102, extra: true });
      link.send({ cmd: "status", req: 9103 });
      const end = Date.now() + 5000;
      while (frames.length < 3 && !closed && Date.now() < end) await new Promise(r => setTimeout(r, 20));
      link.close();
      return { frames, closed };
    })()` ) as { frames: Array<{ id: number; result: { Err?: { code: string } } }>; closed: boolean };
    assert.equal(result.closed, false, "refusals leave the link open");
    assert.equal(result.frames.length, 3, "both refusals and the subsequent status reply arrive");
    for (const id of [9101, 9102]) assert.equal(result.frames.find((frame) => frame.id === id)?.result.Err?.code, "forbidden");
    assert.ok(result.frames.some((frame) => frame.id === 9103 && frame.result.Err === undefined), "status succeeds afterward");
  });
  await ctx.check("a refused link reads as main's own sentence", async () => {
    const result = await call(`return desktop.openExternal("file:///etc/passwd");`);
    assert.equal(result.error, "The app opens https: links only, not file:");
  });
  await ctx.check("a refused notification reaches the page with main's reason", async () => {
    const result = await call(`return desktop.notify({ title: "Review ready", body: "", route: "/pair/aether:local" });`);
    assert.equal(result.error, "A notification opens only an app route");
  });
  await ctx.check("privileged action sends the confirmed frame", async () => {
    ctx.answers.push(true);
    const result = await call(`return desktop.privileged({ kind: "memory.accept", conn: testConn, memory: "${MEMORY}" });`);
    assert.deepEqual(result.ok, { ok: true });
    const shown = ctx.confirmations.at(-1);
    assert.ok(shown?.detail.includes(`"${MEMORY}"`));
    assert.ok(ctx.daemon.lines.includes(JSON.stringify({ cmd: "accept_memory", id: MEMORY, req: 2 })));
  });
  await ctx.check("privileged action refused when cancelled", async () => {
    ctx.answers.push(false);
    const before = ctx.daemon.lines.filter((line) => line.includes("reject_memory")).length;
    const result = await call(`return desktop.privileged({ kind: "memory.reject", conn: testConn, memory: "${MEMORY}" });`);
    assert.match(result.error ?? "", /Cancelled/);
    assert.equal(ctx.daemon.lines.filter((line) => line.includes("reject_memory")).length, before);
  });
}
