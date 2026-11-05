// Flow 7: a desktop remote window on an https origin that reaches aetherd's real listener, and
// archond behind its /archon mount, through a loopback TLS front (as `tailscale serve` would).
// The origin is allowed by configuration only. Paired as an observer, the window reconnects by
// itself after each daemon restarts, and every operate command is refused in the UI, on the wire
// and on the daemons, which record the device as an observer.

import { mkdtemp, readFile, rm } from "node:fs/promises";
import path from "node:path";
import { AETHER_OPERATE } from "../../../../apps/desktop/src/security/classes.ts";
import { createAgent, enqueueItem, queuePaused } from "../../../harness/agents.ts";
import { cli } from "../../../harness/daemons.ts";
import { tempBase } from "../../../harness/env.ts";
import { connection } from "../../../harness/pair.ts";
import { record, until } from "../../../harness/process.ts";
import { expect, test, type Page } from "../../../harness/fixtures.ts";
import type { Stack } from "../../../harness/stack.ts";
import { launchDesktop, widen, type DesktopApp } from "./electron.ts";
import { startFront, type TlsFront } from "./front.ts";
import { aetherConn, aetherDevices, archonDevices, route, setPaused, snapshot } from "./state.ts";
import { installTap, probe, type ProbeReply } from "./tap.ts";

/** The daemon's own access corpus: one parseable frame per command, with its class. */
const COMMAND_CASES = path.resolve(import.meta.dirname, "..", "..", "..", "..", "..", "crates", "aether-daemon", "src", "authz", "command_cases.json");
const DEVICE = "E2E remote window";
const AGENT = "Remote window target";
const ITEM = "Remote window target item";
const PAIR_MS = 20_000;
const RECONNECT_MS = 45_000;
/** What aetherd answers an observer's operate command (crates/aether-daemon/src/authz.rs). */
const AETHER_REFUSAL = { code: "forbidden", message: /needs the operator role/ };
/** What archond's listener answers an observer's change. */
const ARCHON_REFUSAL_CODE = "forbidden";

test.describe.configure({ mode: "serial" });

let dir: string;
let front: TlsFront;
let app: DesktopApp;
let remote: Page;
let pausedBefore: boolean;
let agentId: string;
let itemId: string;
const pageErrors: string[] = [];

async function publicOrigins(stack: Stack): Promise<{ revision: number; settings: Record<string, unknown> }> {
  const { reply, data } = await stack.aether.request({ cmd: "get_remote" });
  expect(reply, "get_remote answers remote").toBe("remote");
  return { revision: data.revision as number, settings: data.settings as Record<string, unknown> };
}

/** The front's origin as aetherd's public origin and an archond trusted origin, by configuration. */
async function allowFront(stack: Stack): Promise<void> {
  await stack.archon.configure(stack.aether.origin, stack.layout.aether.socket, [front.origin]);
  await stack.restart("archon");
  await cli(stack.binaries.aether, ["--local", "remote", "enable", "--origin", front.origin], stack.layout);
  expect((await publicOrigins(stack)).settings.public_origins, "aetherd serves the front's origin").toEqual([front.origin]);
  const archon = await stack.archon.request({ cmd: "remote_status" });
  expect(archon.data.trusted_origins, "archond trusts the front's origin").toEqual([stack.aether.origin, front.origin]);
}

async function restoreOrigins(stack: Stack): Promise<void> {
  const { revision, settings } = await publicOrigins(stack);
  await stack.aether.request({ cmd: "configure_remote", settings: { ...settings, public_origins: [] }, expected_revision: revision });
  expect((await publicOrigins(stack)).settings.public_origins, "aetherd is back to loopback only").toEqual([]);
  await stack.archon.configure(stack.aether.origin, stack.layout.aether.socket);
  await stack.restart("archon");
}

/** Opens the origin's remote window the way the app does: listed in remote.origins, then openRemote. */
async function openRemoteWindow(desktop: DesktopApp, origin: string): Promise<Page> {
  const outcome = await desktop.window.evaluate(async (target) => {
    const bridge = (window as unknown as { desktop: { setPreference(key: string, value: unknown): Promise<void>; openRemote(origin: string): Promise<void> } }).desktop;
    await bridge.setPreference("remote.origins", [target]);
    return bridge.openRemote(target).then(() => "opened", (error: Error) => error.message);
  }, origin);
  expect(outcome, "the bridge opened the remote window").toBe("opened");
  const windows = () => desktop.electron.windows().filter((page) => page.url().startsWith(`${origin}/`));
  await expect.poll(() => windows().length, { message: "Playwright sees the remote window" }).toBe(1);
  const page = windows()[0] as Page;
  await widen(page);
  return page;
}

async function pairAetherObserver(page: Page, stack: Stack): Promise<string> {
  const pairing = await stack.aether.pairing("observer");
  expect(pairing.url, "the pairing URL names the public origin first").toBe(`${front.origin}/pair#code=${pairing.code}`);
  await page.goto(pairing.url);
  await expect(page.getByLabel("Pairing code")).toHaveValue(pairing.code);
  await page.getByLabel("Name for this device").fill(DEVICE);
  await page.getByRole("button", { name: "Pair" }).click();
  const claimed = await until(() => stack.aether.request({ cmd: "get_pairing", id: pairing.id }), (reply) => reply.data.state === "claimed",
    "the remote window's claim on aetherd", PAIR_MS);
  const phrase = String((claimed.data.candidate as { phrase?: unknown } | undefined)?.phrase);
  await expect(page.getByText(phrase, { exact: true })).toBeVisible();
  const enrolled = await stack.aether.request({ cmd: "confirm_pairing", id: pairing.id, accept: true });
  expect(enrolled.data.state).toBe("enrolled");
  await expect(page.getByText("Paired with Aether")).toBeVisible({ timeout: PAIR_MS });
  return String((enrolled.data.device as { id?: unknown }).id);
}

async function pairArchonObserver(page: Page, stack: Stack): Promise<void> {
  const { code } = await stack.archon.pairing("observer");
  await page.goto(`${front.origin}/#/pair/archon`);
  await page.getByLabel("Pairing code").fill(code);
  await page.getByLabel("Name for this device").fill(DEVICE);
  await stack.archon.claimSlot();
  await page.getByRole("button", { name: "Pair" }).click();
  await expect(page.getByText("Paired with Archon")).toBeVisible({ timeout: PAIR_MS });
}

async function bothReady(page: Page, timeout = PAIR_MS): Promise<void> {
  await expect(connection(page, "aether")).toHaveAttribute("data-state", "ready", { timeout });
  await expect(connection(page, "archon")).toHaveAttribute("data-state", "ready", { timeout });
}

test.beforeAll(async ({ stack }) => {
  pausedBefore = await queuePaused(stack);
  await setPaused(stack, true);
  agentId = await createAgent(stack, AGENT, "Stay idle while an observer looks.", false);
  itemId = await enqueueItem(stack, ITEM);
  dir = await mkdtemp(path.join(tempBase(), "tf-"));
  record("roots", dir);
  front = await startFront(dir, stack.aether.ports.aether);
  await allowFront(stack);
  app = await launchDesktop({
    aetherSocket: stack.layout.aether.socket, archonSocket: stack.layout.archon.socket,
    trust: { origin: front.origin, fingerprint: front.fingerprint },
  });
  remote = await openRemoteWindow(app, front.origin);
  remote.on("pageerror", (error) => pageErrors.push(error.message));
  await installTap(remote);
});

test.afterAll(async ({ stack }) => {
  // Every step runs, so one failure leaves no other setting behind.
  const steps = [() => app?.close(), () => restoreOrigins(stack), () => front?.close(), () => rm(dir, { recursive: true, force: true }),
    () => setPaused(stack, pausedBefore)];
  const failures: unknown[] = [];
  for (const step of steps) await Promise.resolve().then(step).catch((error: unknown) => { failures.push(error); });
  if (failures.length > 0) throw new AggregateError(failures, "the remote window flow did not restore the stack");
});

test.afterEach(() => {
  expect(pageErrors.splice(0), "the remote window threw no page error").toEqual([]);
});

test("the remote window pairs with both daemons as an observer through the front", async ({ stack }) => {
  const device = await pairAetherObserver(remote, stack);
  await pairArchonObserver(remote, stack);
  await remote.goto(`${front.origin}/#/fleet`);
  await bothReady(remote);
  expect(front.accepted(), "the window reached the daemons through the front").toBeGreaterThan(0);
  expect(await aetherDevices(stack), "aetherd recorded the window as an observer").toContainEqual({ id: device, name: DEVICE, role: "observer" });
  expect((await archonDevices(stack)).filter((row) => row.name === DEVICE).map((row) => row.role), "archond recorded the window as an observer")
    .toEqual(["observer"]);
});

test("the remote window reconnects by itself after each daemon restarts", async ({ stack }) => {
  await remote.evaluate(() => { (window as { e2eNoReload?: boolean }).e2eNoReload = true; });
  for (const daemon of ["aether", "archon"] as const) {
    const sockets = await remote.evaluate(() => (window as unknown as { e2eTap: { sockets: unknown[] } }).e2eTap.sockets.length);
    await stack.restart(daemon);
    await expect.poll(() => remote.evaluate(() => (window as unknown as { e2eTap: { sockets: unknown[] } }).e2eTap.sockets.length),
      { message: `the window opened a new socket after ${daemon} restarted`, timeout: RECONNECT_MS }).toBeGreaterThan(sockets);
    await bothReady(remote, RECONNECT_MS);
  }
  expect(await remote.evaluate(() => (window as { e2eNoReload?: boolean }).e2eNoReload), "the window was not reloaded").toBe(true);
});

test("as an observer the remote window offers no operate control", async ({ stack }) => {
  await route(remote, `#/controls/${encodeURIComponent(await aetherConn(stack))}/${agentId}`);
  const controls = remote.getByRole("region", { name: `Controls for ${AGENT}` });
  await controls.getByRole("tab", { name: "Model & limits" }).click();
  await expect(controls.getByRole("button", { name: "Rename", exact: true })).toBeDisabled();
  await route(remote, "#/archon/queue");
  await expect(remote.locator(".queue-pause strong")).toHaveText("Paused");
  await expect(remote.getByRole("button", { name: "Resume new work" })).toBeDisabled();
  await remote.getByRole("region", { name: "Queue" }).getByRole("link", { name: ITEM, exact: true }).click();
  await expect(remote.locator("#item-title")).toHaveText(ITEM);
  await expect(remote.getByRole("group", { name: "Item actions" }).getByRole("button", { name: "Cancel", exact: true })).toBeDisabled();
});

/** Every operate case of the daemon's access corpus: a frame aetherd parses, so only the role can refuse it. */
async function aetherOperateFrames(): Promise<Array<Record<string, unknown>>> {
  const cases = JSON.parse(await readFile(COMMAND_CASES, "utf8")) as Array<{ access: string; command: Record<string, unknown> }>;
  const frames = cases.filter((entry) => entry.access === "operate").map((entry) => entry.command);
  const sent = new Set(frames.map((frame) => String(frame.cmd)));
  expect([...AETHER_OPERATE].filter((cmd) => !sent.has(cmd)), "the corpus covers every command of AETHER_OPERATE").toEqual([]);
  return frames;
}

function archonChangeFrames(item: string): Array<Record<string, unknown>> {
  const ref = { host: "github.com", repo: "e2e/never-called", number: 1, kind: "issue" };
  return [
    { cmd: "enqueue", repo: null, pipeline: "ask", title: "Observer enqueue", brief: "Never queued.", priority: "normal", budget_usd: null, idempotency_key: "e2e-observer" },
    { cmd: "cancel_item", item, reason: null },
    { cmd: "retry_item", item, model_plan: null },
    { cmd: "pause", scope: "global", hard: false },
    { cmd: "resume", scope: "global", hard: false },
    { cmd: "link_issue", item, ref, relation: "mentioned" },
    { cmd: "unlink_issue", item, ref, relation: "mentioned" },
    { cmd: "import_issue", ref, pipeline: null, priority: null },
  ];
}

function refusals(replies: readonly ProbeReply[]): Array<{ cmd: string; code: string | undefined }> {
  return replies.map(({ cmd, code }) => ({ cmd, code }));
}

test("on the wire every operate command sent over the window's own connections is refused, and nothing changed", async ({ stack }) => {
  const before = await snapshot(stack);
  const aether = await probe(remote, "aether", await aetherOperateFrames(), 1);
  expect(aether.filter((reply) => reply.ok), "aetherd ran no operate command").toEqual([]);
  expect(refusals(aether), "aetherd refused each for the role").toEqual(aether.map(({ cmd }) => ({ cmd, code: AETHER_REFUSAL.code })));
  for (const reply of aether) expect(reply.message ?? "", `${reply.cmd} is refused for the observer role`).toMatch(AETHER_REFUSAL.message);

  const archon = await probe(remote, "archon", archonChangeFrames(itemId), 2);
  expect(archon.filter((reply) => reply.ok), "archond made no change").toEqual([]);
  expect(refusals(archon), "archond refused each change").toEqual(archon.map(({ cmd }) => ({ cmd, code: ARCHON_REFUSAL_CODE })));

  expect(await snapshot(stack), "agents, items and the pause are as they were").toEqual(before);
  expect((await aetherDevices(stack)).filter((row) => row.name === DEVICE).map((row) => row.role), "aetherd still records an observer").toEqual(["observer"]);
  expect((await archonDevices(stack)).filter((row) => row.name === DEVICE).map((row) => row.role), "archond still records an observer").toEqual(["observer"]);
  await bothReady(remote);
});
