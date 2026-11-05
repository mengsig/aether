// Flow 5: Noesis off, then on, in the desktop app window. Off, the Learning screens show the off
// frame and reach nothing; turning it on is host-only, so the window sends it as main's privileged
// action behind a native confirmation, and the screens then work. It runs on a stack of its own:
// specs/noesis asserts the shared stack never makes noesis.db.

import { expect, test } from "../../../harness/fixtures.ts";
import { connection } from "../../../harness/pair.ts";
import { startAudit, type Audit } from "../../../harness/audit.ts";
import { binariesFromEnv, startStack, type Stack } from "../../../harness/stack.ts";
import { noesisCommands, noesisFiles, noesisStatus } from "../../noesis/noesis.ts";
import { launchDesktop, type DesktopApp } from "./electron.ts";
import { route } from "./state.ts";

const LEARNING = ["#/learn", "#/learn/graph", "#/learn/me"] as const;

test.describe.configure({ mode: "serial" });

let own: Stack;
let audit: Audit;
let app: DesktopApp;
const pageErrors: string[] = [];

/** The `cmd` of every frame the app window sent through the bridge, in order. */
async function bridgeCommands(desktop: DesktopApp): Promise<string[]> {
  return (await desktop.calls())
    .filter((call) => call.method === "request" || call.method === "subscribe")
    .map((call) => String((call.args[1] as { cmd?: unknown } | undefined)?.cmd));
}

test.beforeAll(async () => {
  test.setTimeout(180_000);
  own = await startStack(await binariesFromEnv());
  audit = startAudit({
    root: () => own.layout.root,
    binaries: [own.binaries.aether, own.binaries.archon],
    daemons: () => [own.aether.pid, own.archon.pid].filter((pid): pid is number => pid !== undefined),
    clients: () => [process.pid],
  });
  app = await launchDesktop({ aetherSocket: own.layout.aether.socket, archonSocket: own.layout.archon.socket });
  app.window.on("pageerror", (error) => pageErrors.push(error.message));
  await expect(connection(app.window, "aether")).toHaveAttribute("data-state", "ready", { timeout: 30_000 });
});

test.afterAll(async () => {
  const failures: unknown[] = [];
  for (const step of [() => app?.close(), () => own?.stop()]) await Promise.resolve().then(step).catch((error: unknown) => { failures.push(error); });
  audit?.stop();
  if (failures.length > 0) throw new AggregateError(failures, "the Noesis flow's own stack did not stop cleanly");
  expect(audit.samples, "the private stack's audit sampled").toBeGreaterThan(0);
  expect(audit.violations, "the private stack stayed hermetic").toEqual([]);
});

test.afterEach(() => {
  expect(pageErrors.splice(0), "the app window threw no page error").toEqual([]);
});

test("with Noesis off the Learning screens show the off frame and start nothing", async () => {
  expect((await noesisStatus(own)).enabled, "the stack starts with Noesis off").toBe(false);
  const requests = own.engine.requests.length;
  for (const screen of LEARNING) {
    await route(app.window, screen);
    await expect(app.window.getByTestId("noesis-off"), `${screen} shows the off frame`).toBeVisible();
  }
  expect(own.engine.requests.length, "no model was asked").toBe(requests);
  expect(await noesisFiles(own), "no Noesis file was made").toEqual([]);
  expect((await noesisStatus(own)).enabled, "Noesis is still off").toBe(false);
  const sent = await bridgeCommands(app);
  expect(sent.length, "the window talked to the daemons through the bridge").toBeGreaterThan(0);
  expect(noesisCommands(sent).filter((cmd) => cmd !== "noesis_status"), "the window read nothing of Noesis but its status").toEqual([]);
  expect((await app.calls()).filter((call) => call.method === "privileged"), "no privileged action ran").toEqual([]);
});

test("turning Noesis on in the settings sheet goes through main's native confirmation", async () => {
  await route(app.window, "#/learn");
  await app.window.getByTestId("noesis-off").getByRole("button", { name: "Open Noesis settings" }).click();
  const sheet = app.window.getByRole("dialog", { name: "Noesis settings" });
  const toggle = sheet.getByRole("switch", { name: /Turn on Noesis/ });
  await expect(toggle, "the host may turn Noesis on").toBeEnabled();
  await app.answer(true);
  await toggle.check();
  await sheet.getByRole("button", { name: "Save changes" }).click();

  await expect.poll(async () => (await noesisStatus(own)).enabled, { message: "aetherd turned Noesis on" }).toBe(true);
  expect(await noesisFiles(own), "turning Noesis on made noesis.db").toContain("noesis.db");
  const shown = await app.confirmations();
  expect(shown.map((confirmation) => confirmation.title), "main asked once, natively").toEqual(["Noesis settings"]);
  expect(shown[0]?.detail ?? "", "the confirmation names the change").toMatch(/enabled: .*false.* → .*true/);
  const privileged = (await app.calls()).filter((call) => call.method === "privileged").map((call) => call.args[0]);
  expect(privileged, "the window sent the switch as main's privileged action").toEqual([expect.objectContaining({ kind: "noesis.configure" })]);
});

test("with Noesis on the Learning screens work", async () => {
  const requests = own.engine.requests.length;
  await route(app.window, "#/learn");
  await expect(app.window.getByRole("region", { name: "Your sessions" }).getByText("No sessions yet")).toBeVisible();
  for (const screen of LEARNING.slice(1)) {
    await route(app.window, screen);
    await expect(app.window.locator(".learn-body"), `${screen} renders its screen`).toBeVisible();
    await expect(app.window.getByTestId("noesis-off"), `${screen} has no off frame`).toHaveCount(0);
  }
  const sent = noesisCommands(await bridgeCommands(app));
  for (const read of ["noesis_list_sessions", "noesis_graph", "noesis_learner"]) expect(sent, `the screens read with ${read}`).toContain(read);
  expect(own.engine.requests.length, "opening the screens asked no model").toBe(requests);
});
