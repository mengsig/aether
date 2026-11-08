// Flow 6c, the packaged app's native surfaces, each as the audit reproduced it: outside links
// and reports open (UX-034, UX-031), deep links navigate (UX-054), waiting agents alert (UX-045),
// pairing asks where Aether runs (UX-044) and remote access shows its state (UX-002). It drives
// only the packaged build (E2E_DESKTOP_PACKAGED); E2E_SHOTS keeps 1440x900 shots in both themes.

import assert from "node:assert/strict";
import { createAgent, enqueueItem } from "../../../harness/agents.ts";
import { connection } from "../../../harness/pair.ts";
import { until } from "../../../harness/process.ts";
import type { Stack } from "../../../harness/stack.ts";
import { expect, test, type Page } from "../../../harness/fixtures.ts";
import { fixtureRepository, installGhStub, OWNER_REPO } from "../gh-stub.ts";
import { startPrivateStack, stopPrivateStack, type PrivateStack } from "../private.ts";
import { focusWindow, minimizeWindow, pressKeys, windowTitles } from "./native-host.ts";
import { askingScript, installLavishStub, publishReport, shoot, shootScreen, startReportServer, VIEWPORT, type ReportServer } from "./native-fixtures.ts";
import { launchPackaged, packagedBinary, type PackagedApp } from "./packaged.ts";
import { route, setPaused } from "./state.ts";

const ISSUE = 7;
const ISSUE_URL = `https://github.com/${OWNER_REPO}/issues/${ISSUE}`;
const REPORT_TITLE = "Proposal: native links";
const GONE_TITLE = "Proposal: a stopped server";
const TAILNET_TITLE = "Proposal: private tailnet report";
const MARKER = "native-e2e-asks";
const QUESTIONS = ["Which summary style do you want?", "Should the summary name every file?"] as const;

const binary = packagedBinary();
test.skip(binary === undefined, "the native journeys drive the packaged build; set E2E_DESKTOP_PACKAGED");
test.describe.configure({ mode: "serial" });

let running: PrivateStack | undefined;
let reports: ReportServer | undefined;
let app: PackagedApp | undefined;
let item = "";
const pageErrors: string[] = [];

function stack(): Stack {
  assert.ok(running !== undefined, "the private stack is up");
  return running.stack;
}

function desktop(): PackagedApp {
  assert.ok(app !== undefined, "the packaged app is up");
  return app;
}

function window(): Page {
  return desktop().window;
}

/** The toast that says how an outside link went. */
const toast = (page: Page, text: string | RegExp) => page.locator(".ui-toast").filter({ hasText: text });

async function launch(args: readonly string[] = []): Promise<void> {
  assert.ok(binary !== undefined && app === undefined, "one packaged app at a time");
  const { aether, archon } = stack().layout;
  app = await launchPackaged(binary, { aether: aether.socket, archon: archon.socket },
    { args: ["--host-resolver-rules=MAP report.tailnet.test 127.0.0.1", "--no-proxy-server", ...args] });
  app.window.on("pageerror", (error) => pageErrors.push(error.message));
  await app.window.setViewportSize(VIEWPORT);
  await app.window.emulateMedia({ colorScheme: "light", reducedMotion: "reduce" });
  await expect(connection(app.window, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
  await expect(connection(app.window, "archon")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
}

/** Answers the native confirmation titled `title` on the private compositor's keyboard. */
async function answerNative(title: string, confirm: boolean, shot?: string, tabs = 2): Promise<void> {
  const { compositor } = desktop();
  await until(() => windowTitles(compositor), (titles) => titles.includes(title), `the native "${title}" dialog`, 15_000);
  if (shot !== undefined) await shootScreen(compositor, shot);
  await focusWindow(compositor, title);
  await pressKeys(compositor, confirm ? [...Array<string>(tabs).fill("Tab"), "Return"] : ["Escape"]);
  await until(() => windowTitles(compositor), (titles) => !titles.includes(title), `the "${title}" dialog to close`, 15_000);
}

test.beforeAll(async () => {
  test.setTimeout(240_000);
  running = await startPrivateStack();
  await setPaused(stack(), true);
  await installGhStub(stack().layout, [{ number: ISSUE, title: "Native links open", state: "open", body: "Open me on GitHub." }]);
  const repo = await fixtureRepository(stack().layout);
  const added = await stack().archon.request({ cmd: "repo_add", path: repo, name: "e2e-repo" });
  expect(added.reply, `archond registers the fixture repository: ${JSON.stringify(added)}`).not.toBe("error");
  await installLavishStub(stack());
  reports = await startReportServer(REPORT_TITLE);
  item = await enqueueItem(stack(), "Native journeys item");
  await publishReport(stack(), repo, item, REPORT_TITLE, reports.url);
  await publishReport(stack(), repo, item, TAILNET_TITLE, reports.tailnet);
  await publishReport(stack(), repo, item, GONE_TITLE, reports.gone);
  await launch();
});

test("a daemon report on a tailnet address opens, while an unrelated http URL is refused", async () => {
  assert.ok(reports !== undefined, "the private report server is running");
  const page = window();
  await route(page, `#/archon/items/${item}`);
  await page.locator(".item-link").filter({ hasText: TAILNET_TITLE }).getByRole("link", { name: "Open" }).click();
  await expect(toast(page, "The report opened in its own window")).toBeVisible();
  let viewer: Page | undefined;
  await expect.poll(() => {
    viewer = desktop().browser.contexts().flatMap((context) => context.pages()).find((candidate) => candidate.url() === reports?.tailnet);
    return viewer !== undefined;
  }, { message: "the daemon-returned tailnet report opens" }).toBe(true);
  assert.ok(viewer !== undefined, "the report viewer exists");
  await expect(viewer.getByRole("heading", { name: REPORT_TITLE })).toBeVisible();
  await shootScreen(desktop().compositor, "report-tailnet-window");
  const refused = await page.evaluate(async (url) => {
    const bridge = (window as unknown as { desktop: { openExternal(value: string): Promise<void> } }).desktop;
    return bridge.openExternal(url).then(() => "opened", (error: Error) => error.message);
  }, `http://unrelated.tailnet.test:${new URL(reports.tailnet).port}/session/e2e`);
  expect(refused).toMatch(/only https|not on this computer/i);
  expect(desktop().native.opened()).not.toContain(reports.tailnet);
  await viewer.close();
});

test.afterAll(async () => {
  try {
    await app?.close();
    await reports?.close();
  } finally {
    await stopPrivateStack(running);
  }
});

test.afterEach(() => {
  expect(pageErrors.splice(0), "the app window threw no page error").toEqual([]);
});

test("UX-034: Open on GitHub opens the browser, by click and by keyboard, and says so", async () => {
  const page = window();
  await route(page, `#/github/github.com/${OWNER_REPO}/${ISSUE}`);
  const link = page.getByRole("group", { name: "Actions" }).getByRole("link", { name: "Open on GitHub" });
  await expect(link).toBeVisible();
  await link.click();
  await expect(toast(page, "Opened github.com in your browser")).toBeVisible();
  await expect.poll(() => desktop().native.opened(), { message: "main handed the issue to the browser" }).toEqual([ISSUE_URL]);
  await shoot(page, "ux034-github-opened");
  await link.focus();
  await page.keyboard.press("Enter");
  await expect.poll(() => desktop().native.opened(), { message: "Enter on the link opens it too" }).toEqual([ISSUE_URL, ISSUE_URL]);
  expect(page.url(), "the app window stayed on its page").toContain(`#/github/github.com/${OWNER_REPO}/${ISSUE}`);
});

test("UX-031: a published report opens in its own window; a stopped server says why", async () => {
  const page = window();
  await route(page, `#/archon/items/${item}`);
  const rows = page.locator(".item-link");
  const open = rows.filter({ hasText: REPORT_TITLE }).getByRole("link", { name: "Open" });
  await open.scrollIntoViewIfNeeded();
  await shoot(page, "ux031-reports");
  await open.click();
  await expect(toast(page, "The report opened in its own window")).toBeVisible();
  let viewer: Page | undefined;
  await expect.poll(() => {
    viewer = desktop().browser.contexts().flatMap((context) => context.pages()).find((candidate) => candidate.url() === reports?.url);
    return viewer !== undefined;
  }, { message: "a report window shows the report's page" }).toBe(true);
  assert.ok(viewer !== undefined, "the report window was found");
  await expect(viewer.getByRole("heading", { name: REPORT_TITLE })).toBeVisible();
  expect(await viewer.evaluate(() => typeof (globalThis as { desktop?: unknown }).desktop), "a report page gets no bridge").toBe("undefined");
  expect(await viewer.evaluate(() => Notification.permission), "a report page cannot notify around main's limit").toBe("denied");
  expect(desktop().native.opened(), "a local report never goes to the browser").not.toContain(reports?.url);
  await shootScreen(desktop().compositor, "ux031-report-window");
  await rows.filter({ hasText: GONE_TITLE }).getByRole("link", { name: "Open" }).click();
  await expect(toast(page, /Could not open the report: The report at http:\/\/127\.0\.0\.1:\d+ did not load; is its server still running\?/)).toBeVisible();
  await shoot(page, "ux031-report-gone");
});

test("UX-045: Settings has desktop alerts and a test notification that reaches the system", async () => {
  const page = window();
  await route(page, "#/settings/notifications");
  const section = page.getByRole("region", { name: "Desktop alerts" });
  for (const name of ["Desktop alerts", "Questions", "Approvals"]) await expect(section.getByRole("switch", { name: new RegExp(`^${name}`) })).toBeChecked();
  const front = section.getByRole("switch", { name: /^Also while Aether is in front/ });
  await expect(front).not.toBeChecked();
  await shoot(page, "ux045-settings");
  await section.getByRole("switch", { name: /^Approvals/ }).click();
  await expect(section.getByText("Approvals off")).toBeVisible();
  await section.getByRole("switch", { name: /^Approvals/ }).click();
  await expect(section.getByText("Approvals on")).toBeVisible();
  await section.getByRole("button", { name: "Send test notification" }).click();
  await expect(section.getByText(/Test notification sent\./)).toBeVisible();
  await expect.poll(() => desktop().native.notes().map((note) => note.title), { message: "the system was asked to show the test" })
    .toEqual(["Aether alerts are on"]);
});

test("UX-045: a real agent's notification restores the packaged app and focuses its question", async () => {
  test.setTimeout(120_000);
  const page = window();
  await route(page, "#/inbox");
  await expect(page.getByText(/^Nothing on .* needs you$/)).toBeVisible();
  await route(page, "#/fleet");
  await minimizeWindow(desktop().compositor, await page.title());
  await shootScreen(desktop().compositor, "notification-before-click");
  stack().engine.setScript(askingScript(MARKER, QUESTIONS));
  const agent = await createAgent(stack(), "Native asker", `Ask the operator two things (${MARKER}).`);
  const alerted = await until(async () => desktop().native.notes(), (notes) => notes.some((note) => note.body === QUESTIONS[0]), "the first question's alert", 60_000);
  const first = alerted.find((note) => note.body === QUESTIONS[0]);
  assert.ok(first !== undefined, "the first question alerted");
  expect(first.title).toBe("Native asker asks");
  expect(first.actions).toContain("default");
  await desktop().native.click(first.id);
  await expect.poll(() => page.evaluate(() => location.hash), { message: "the click opens the inbox" }).toBe("#/inbox");
  const card = page.locator(`[data-question*="/${agent}:"]`).filter({ hasText: QUESTIONS[0] });
  await expect(card.getByRole("textbox", { name: "Answer Native asker" })).toBeFocused();
  await shootScreen(desktop().compositor, "notification-answer-focused");
});

test("UX-054: links from a second launch navigate the open window, again after moving away", async () => {
  const page = window();
  const logStart = desktop().mainLog().length;
  await route(page, "#/inbox");
  for (const [link, hash] of [["%2Ffleet", "#/fleet"], ["%2Farchon%2Fqueue", "#/archon/queue"]] as const) {
    expect(await desktop().again([`aether-app://open?route=${link}`]), "the second launch hands the link over and exits cleanly").toBe(0);
    await expect.poll(() => page.evaluate(() => location.hash), { message: `the link opens ${hash}` }).toBe(hash);
  }
  await route(page, "#/activity");
  expect(await desktop().again(["aether-app://open?route=%2Ffleet"])).toBe(0);
  await expect.poll(() => page.evaluate(() => location.hash), { message: "the retried link leaves Activity for Fleet" }).toBe("#/fleet");
  await shoot(page, "ux054-fleet-from-link", ["light"]);
  for (const log of desktop().secondInstanceLogs()) expect(log, "second instances exit without a failed app navigation").not.toContain("ERR_FAILED");
  expect(desktop().mainLog().slice(logStart), "the primary instance logs no failed navigation during handoff").not.toContain("ERR_FAILED");
});

for (const daemon of ["aether", "archon"] as const) {
  test(`UX-044: ${daemon} pairing asks where it runs, checks the host address and offers a retry`, async () => {
    const page = window();
    await route(page, `#/pair/${daemon}`);
    await expect(page.getByRole("heading", { name: /^Where does (Aether|Archon) run\?$/ })).toBeVisible();
    const where = page.getByRole("group", { name: /^Where (Aether|Archon) runs$/ });
    await where.getByRole("radio", { name: /This computer/ }).check();
    await expect(page.getByRole("link", { name: /Set up (Aether|Archon) on this computer|Open Connections/ })).toBeVisible();
    await shoot(page, `ux044-${daemon}-this-computer`);
    await where.getByRole("radio", { name: /Another computer/ }).check();
    const address = page.getByRole("textbox", { name: "Host address" });
    await address.fill("http://box.example");
    await page.getByRole("button", { name: "Open the host" }).click();
    await expect(page.getByText("Another computer is reached over https: only, not http:")).toBeVisible();
    await shoot(page, `ux044-${daemon}-invalid-address`);
    await address.fill("https://127.0.0.1:1");
    await page.getByRole("button", { name: "Open the host" }).click();
    const failed = page.getByRole("alert").filter({ hasText: "Could not open https://127.0.0.1:1" });
    await expect(failed).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText(/Unexpected end of JSON input/), "no raw parser error reaches the person").toHaveCount(0);
    await shoot(page, `ux044-${daemon}-open-failed`);
    await failed.getByRole("button", { name: "Retry" }).click();
    await expect(page.getByRole("alert").filter({ hasText: "Could not open https://127.0.0.1:1" })).toBeVisible({ timeout: 30_000 });
  });
}

test("UX-002: Remote shows its state with one control, and verifies every change", async () => {
  test.setTimeout(120_000);
  const page = window();
  await route(page, "#/settings/remote");
  const verified = page.getByRole("status").filter({ hasText: /^Verified|Checking remote access/ });
  await expect(page.getByText(/^Address: /)).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".ui-badge").filter({ hasText: /^(On|Off)$/ }).first()).toHaveText("On");
  await expect(page.getByRole("button", { name: /remote access$/ })).toHaveCount(1);
  await shoot(page, "ux002-remote-on");
  for (const [action, after] of [["Turn off remote access", "Verified: remote access is off"], ["Turn on remote access", /^Verified: remote access is on/]] as const) {
    for (const tabs of [2, 1, 0]) {
      await page.getByRole("button", { name: action }).click();
      await page.getByRole("dialog").getByRole("button", { name: action, exact: true }).click();
      await answerNative("Remote access", true, action === "Turn off remote access" ? "ux002-native-confirm" : undefined, tabs);
      await expect.poll(async () => await verified.isVisible() || await page.getByRole("region", { name: "Remote access" }).getByRole("alert").filter({ hasText: "Cancelled" }).isVisible()).toBe(true);
      if (await verified.isVisible()) break;
    }
    await expect(verified).toHaveText(after, { timeout: 30_000 });
    await expect(page.getByRole("button", { name: /remote access$/ })).toHaveCount(1);
  }
  await shoot(page, "ux002-remote-verified");
  await page.getByRole("button", { name: "Turn off remote access" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Turn off remote access", exact: true }).click();
  await answerNative("Remote access", false);
  await expect(page.locator(".ui-badge").filter({ hasText: /^(On|Off)$/ }).first(), "a cancelled change leaves it on").toHaveText("On");
});

test("UX-054: a link that launches the app opens its screen", async () => {
  await app?.close();
  app = undefined;
  await launch(["aether-app://open?route=%2Ffleet"]);
  await expect.poll(() => window().evaluate(() => location.hash), { message: "the launch link opens Fleet" }).toBe("#/fleet");
  await shoot(window(), "ux054-launch-link", ["light"]);
});
