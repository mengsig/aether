// The headless lifecycle harness: starts the real desktop main process offscreen with a stub
// `aether` first on its PATH, drives managed start and deep links from the page, prints one JSON
// line of results, then quits or crashes as the runner asks. The runner owns the stub daemon.

import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import path from "node:path";
import { app, type WebContents } from "electron";
import { registerAppScheme, startDesktop, type Desktop } from "../../../src/main/start.ts";
import { DeepLinkRouter } from "../../../src/native/deep-links.ts";

export const RESULT_PREFIX = "DESKTOP-LIFECYCLE-RESULTS ";
const SCRIPT_TIMEOUT_MS = 40_000;

interface CheckResult { readonly name: string; readonly ok: boolean; readonly error?: string }

/** Runs `script` in the page, which reaches main only through window.desktop. */
async function page(contents: WebContents, script: string): Promise<unknown> {
  let timer: NodeJS.Timeout | undefined;
  const deadline = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new Error(`page script timed out: ${script.slice(0, 80)}`)), SCRIPT_TIMEOUT_MS);
  });
  try {
    return await Promise.race([contents.executeJavaScript(script, true), deadline]);
  } finally {
    clearTimeout(timer);
  }
}

/** The page's hash once it equals `want`, within a few seconds; the hash it has otherwise. */
async function hashBecomes(contents: WebContents, want: string): Promise<string> {
  let hash = "";
  for (let attempt = 0; attempt < 50 && hash !== want; attempt += 1) {
    hash = String(await page(contents, "location.hash"));
    if (hash !== want) await new Promise((resolve) => setTimeout(resolve, 100));
  }
  return hash;
}

const settled = (call: string) => `${call}.then((value) => ({ value }), (error) => ({ error: String(error.message) }))`;

async function checks(desktop: Desktop, dir: string): Promise<CheckResult[]> {
  const contents = desktop.appWindow.webContents;
  const pidFile = path.join(dir, "a.pid");
  const results: CheckResult[] = [];
  const check = async (name: string, body: () => Promise<void>) => {
    try {
      await body();
      results.push({ name, ok: true });
    } catch (error) {
      results.push({ name, ok: false, error: error instanceof Error ? (error.stack ?? error.message) : String(error) });
    }
  };
  await check("a link from before the window loaded navigates the page", async () => {
    assert.equal(await hashBecomes(contents, "#/fleet"), "#/fleet");
  });
  const aether = async () => ((await page(contents, "desktop.discover()")) as Array<{ kind: string }>).find((d) => d.kind === "aether");
  await check("managed start is off by default", async () => {
    const answer = await page(contents, settled('desktop.startDaemon("aether")')) as { error?: string };
    assert.match(answer.error ?? "", /Starting daemons from the app is off/);
    assert.equal(existsSync(pidFile), false, "no daemon ran");
    assert.deepEqual(await aether(), { id: "aether:local", kind: "aether", label: "aetherd", reachable: false });
  });
  await check("managed start runs the daemon once the setting is on", async () => {
    await page(contents, 'desktop.setPreference("desktop.managed_start", true)');
    const answer = await page(contents, settled('desktop.startDaemon("aether")')) as { error?: string };
    assert.equal(answer.error, undefined);
    assert.equal(existsSync(pidFile), true, "the stub daemon is listening");
    assert.deepEqual(await aether(), { id: "aether:local", kind: "aether", label: "aetherd", reachable: true, version: "stub-daemon" });
  });
  await check("deep links reach the page and anything else is dropped", async () => {
    await page(contents, "window.links = []; desktop.onDeepLink((route) => window.links.push(route)); true");
    assert.equal(desktop.deepLinks.acceptLink("aether-app://open?route=%2Farchon%2Fqueue"), true);
    assert.equal(desktop.deepLinks.acceptLink("aether-app://open?route=/pair/aether:local"), false);
    assert.equal(desktop.deepLinks.acceptLink("https://example.com/?route=/fleet"), false);
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.deepEqual(await page(contents, "window.links"), ["/archon/queue"]);
    assert.equal(await hashBecomes(contents, "#/archon/queue"), "#/archon/queue", "the page navigated to the link's route");
  });
  return results;
}

const dir = process.env.DESKTOP_LIFECYCLE_DIR ?? "";
const ending = process.env.DESKTOP_LIFECYCLE_EXIT;
if (!path.isAbsolute(dir) || !existsSync(dir)) throw new Error("DESKTOP_LIFECYCLE_DIR names the run's directory");
if (ending !== "quit" && ending !== "crash") throw new Error("DESKTOP_LIFECYCLE_EXIT is quit or crash");
app.setPath("userData", path.join(dir, "user-data"));
app.setPath("logs", path.join(dir, "logs"));
registerAppScheme();
app.whenReady()
  .then(async () => {
    // Only the run's own sockets: the app must never find a real daemon of this machine.
    const env = {
      PATH: `${path.join(dir, "bin")}:/usr/bin:/bin`, HOME: dir,
      AETHER_SOCKET: path.join(dir, "a.sock"), ARCHON_SOCKET: path.join(dir, "r.sock"),
    };
    // A launch-time link: it waits in main until the window loads, then in the preload until the page listens.
    const deepLinks = new DeepLinkRouter();
    assert.equal(deepLinks.acceptLink("aether-app://open?route=%2Ffleet"), true);
    return checks(await startDesktop({ offscreen: true, env, deepLinks }), dir);
  })
  .then((results) => {
    process.stdout.write(`${RESULT_PREFIX}${JSON.stringify(results)}\n`, () => {
      if (ending === "crash") process.crash();
      app.quit();
    });
  }, (error: unknown) => {
    process.stderr.write(`the harness failed: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`);
    app.exit(2);
  });
