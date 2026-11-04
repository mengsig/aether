// The headless Electron security harness: starts the real desktop main process offscreen against
// a fake daemon and a fixture HTTPS listener, runs every check, prints one JSON line and exits.

import { existsSync } from "node:fs";
import path from "node:path";
import { app, session } from "electron";
import { startDesktop, registerAppScheme } from "../../../src/main/start.ts";
import { remotePartition } from "../../../src/security/origins.ts";
import { appIsolationChecks, appWindowChecks, bridgeChecks } from "./app-checks.ts";
import { Context } from "./context.ts";
import { fakeDaemon, makeCertificate, remoteServer, type TlsFixture } from "./fixtures.ts";
import { forgetChecks, openRemoteChecks, remoteIsolationChecks, remoteWindowChecks } from "./remote-checks.ts";

export const RESULT_PREFIX = "DESKTOP-SECURITY-RESULTS ";

/** Test only: these sessions trust the fixture's one certificate; production overrides none. */
function trustFixture(tls: TlsFixture, sessions: readonly Electron.Session[]): void {
  for (const target of sessions) {
    target.setCertificateVerifyProc((request, callback) => {
      callback(request.certificate.fingerprint === tls.fingerprint ? 0 : -3);
    });
  }
}

async function run(dir: string): Promise<Context> {
  const tls = makeCertificate(dir);
  const ctx = new Context(await fakeDaemon(dir), await remoteServer(tls));
  trustFixture(tls, [session.defaultSession, session.fromPartition(remotePartition(ctx.server.origin))]);
  const env = { AETHER_SOCKET: ctx.daemon.socket, ARCHON_SOCKET: path.join(dir, "absent-archond.sock") };
  ctx.desktop = await startDesktop({ offscreen: true, env, confirm: ctx.confirm });
  try {
    await appWindowChecks(ctx);
    await appIsolationChecks(ctx);
    await bridgeChecks(ctx);
    await openRemoteChecks(ctx);
    await remoteWindowChecks(ctx);
    await remoteIsolationChecks(ctx);
    await forgetChecks(ctx);
  } finally {
    await ctx.server.close();
    await ctx.daemon.close();
  }
  if (ctx.answers.length !== 0) throw new Error("a scripted confirmation answer was never used");
  return ctx;
}

// The runner owns and removes the directory once Electron has exited; keep it short, for the socket.
const dir = process.env.DESKTOP_SECURITY_DIR ?? "";
if (!path.isAbsolute(dir) || !existsSync(dir)) throw new Error("DESKTOP_SECURITY_DIR names the run's directory");
app.setPath("userData", path.join(dir, "user-data"));
registerAppScheme();
app.whenReady()
  .then(() => run(dir))
  .then((ctx) => {
    process.stdout.write(`${RESULT_PREFIX}${JSON.stringify(ctx.results)}\n`);
    app.exit(ctx.results.every((result) => result.ok) ? 0 : 1);
  }, (error: unknown) => {
    process.stderr.write(`the harness failed: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`);
    app.exit(2);
  });
