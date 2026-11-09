// Fixtures for the packaged native journeys: a local report server with a Lavish stand-in that
// names its pages, an agent script that asks real questions, and 1440x900 shots in both themes.

import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import http from "node:http";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { promisify } from "node:util";
import { expect, type Page } from "@playwright/test";
import { DEFAULT_REPLY, type Script } from "../../../harness/engine.ts";
import type { Stack } from "../../../harness/stack.ts";

const exec = promisify(execFile);
const SHOTS = process.env.E2E_SHOTS;
export const VIEWPORT = { width: 1440, height: 900 } as const;

export interface ReportServer {
  /** The page a published report shows, on this computer's loopback. */
  readonly url: string;
  readonly tailnet: string;
  /** A loopback URL nothing listens on, for a report whose server is gone. */
  readonly gone: string;
  close(): Promise<void>;
}

async function listen(server: http.Server): Promise<number> {
  await new Promise<void>((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  const address = server.address() as AddressInfo;
  assert.equal(address.address, "127.0.0.1", "the report server listens on loopback only");
  return address.port;
}

/** Serves one proposal page, as a review session does, and finds a port with nothing on it. */
export async function startReportServer(title: string): Promise<ReportServer> {
  const page = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>${title}</title></head>`
    + `<body style="font-family:system-ui;margin:48px"><h1>${title}</h1><p>A proposal page a local report server shows.</p></body></html>`;
  const server = http.createServer((request, response) => {
    response.writeHead(request.url === "/session/e2e" ? 200 : 404, { "Content-Type": "text/html; charset=utf-8" }).end(page);
  });
  const port = await listen(server);
  const probe = http.createServer();
  const free = await listen(probe);
  await new Promise<void>((resolve) => probe.close(() => resolve()));
  return {
    url: `http://127.0.0.1:${port}/session/e2e`,
    tailnet: `http://report.tailnet.test:${port}/session/e2e`,
    gone: `http://127.0.0.1:${free}/session/gone`,
    close: () => new Promise<void>((resolve, reject) => {
      server.closeAllConnections();
      server.close((error) => (error ? reject(error) : resolve()));
    }),
  };
}

/**
 * A `lavish-axi` on the stack's PATH: opening `<html>` answers the session URL kept beside it in
 * `<html>.url`; a poll waits while archond lives, so nothing it starts outlives the stack.
 */
export async function installLavishStub(stack: Stack): Promise<void> {
  const stub = path.join(stack.layout.bin, "lavish-axi");
  assert.ok(String(stack.layout.env.PATH).startsWith(`${stack.layout.bin}:`), "the stack's PATH finds its own bin first");
  await writeFile(stub, [
    "#!/bin/sh",
    'case "$1" in',
    '  poll) while kill -0 "$PPID" 2>/dev/null; do sleep 1; done; exit 0 ;;',
    "  end) printf 'session:\\n  status: ended\\n'; exit 0 ;;",
    "esac",
    '[ -f "$1.url" ] || exit 3',
    "printf 'session:\\n  status: open\\n  url: \"%s\"\\n' \"$(cat \"$1.url\")\"",
    "",
  ].join("\n"), { mode: 0o755 });
  // archond looks for Lavish when it starts.
  await stack.restart("archon");
}

/** Publishes an HTML report on `item` whose review session is `url`; archond takes a page from a registered `repo` only. */
export async function publishReport(stack: Stack, repo: string, item: string, title: string, url: string): Promise<string> {
  assert.ok(path.isAbsolute(repo) && repo.startsWith(`${stack.layout.root}/`), "the report is written in the stack's own repository");
  await mkdir(path.join(repo, "reports"), { recursive: true });
  const file = path.join(repo, "reports", `${title.toLowerCase().replaceAll(/[^a-z0-9]+/g, "-")}.html`);
  await writeFile(file, `<!doctype html><title>${title}</title><h1>${title}</h1>`);
  await writeFile(`${file}.url`, url);
  const { reply, data } = await stack.archon.request({ cmd: "publish_report", item, title, path: file });
  assert.equal(reply, "report", `archond publishes the report: ${JSON.stringify(data)}`);
  assert.equal(data.location, url, "the report's location is its review session");
  return String(data.report);
}

/** An agent whose objective names `marker` asks `questions` one by one, then finishes. */
export function askingScript(marker: string, questions: readonly string[]): Script {
  assert.ok(marker !== "" && questions.length > 0, "an asking agent has a marker and questions");
  return (request) => {
    if (!request.userTexts.some((text) => text.includes(marker))) return DEFAULT_REPLY;
    const asked = request.toolsCalled.filter((tool) => tool === "ask_user").length;
    const next = questions[asked];
    return next === undefined ? "Thanks, that is everything." : { name: "ask_user", arguments: { question: next } };
  };
}

/** Shoots `page` at 1440x900 as `<name>-<theme>.png`, in each theme; does nothing without E2E_SHOTS. */
export async function shoot(page: Page, name: string, themes: ReadonlyArray<"light" | "dark"> = ["light", "dark"]): Promise<void> {
  if (SHOTS === undefined || SHOTS === "") return;
  assert.ok(path.isAbsolute(SHOTS), `E2E_SHOTS names an absolute directory, not ${SHOTS}`);
  assert.deepEqual(page.viewportSize(), VIEWPORT, "every shot is 1440x900");
  for (const theme of themes) {
    await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    await page.screenshot({ path: path.join(SHOTS, `${name}-${theme}.png`) });
  }
  await page.emulateMedia({ colorScheme: "light", reducedMotion: "reduce" });
}

/** Shoots the whole private compositor, native dialogs included; does nothing without E2E_SHOTS. */
export async function shootScreen(compositor: NodeJS.ProcessEnv, name: string): Promise<void> {
  if (SHOTS === undefined || SHOTS === "") return;
  await exec("/usr/bin/grim", [path.join(SHOTS, `${name}.png`)], { env: compositor, timeout: 10_000 });
}
