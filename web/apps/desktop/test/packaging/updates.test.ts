// No path installs an update without the user's confirmation: a recording fake stands in for
// electron-updater, and every answer sequence, failure and reentry is driven through the flow.

import assert from "node:assert/strict";
import { test } from "node:test";
import type { Confirmation } from "../../src/security/confirm.ts";
import { buildInfo } from "../../src/update/build-info.ts";
import {
  ConfirmedStepError, lockDown, UpdateFlow, updatesSupported, type BuildInfo, type Updater,
} from "../../src/update/updates.ts";

const UNSIGNED: BuildInfo = { version: "1.2.11", signed: false, notarized: false, label: "unsigned build" };
const SIGNED: BuildInfo = { version: "1.2.11", signed: true, notarized: true, label: "signed build" };

/** electron-updater as the flow sees it, recording every call; it installs only through quitAndInstall. */
class FakeUpdater implements Updater {
  autoDownload = true;
  autoInstallOnAppQuit = true;
  allowDowngrade = true;
  allowPrerelease = true;
  readonly calls: string[] = [];
  available = true;
  version = "1.3.0";
  downloadFails = false;

  async checkForUpdates() {
    this.calls.push("check");
    return { isUpdateAvailable: this.available, updateInfo: { version: this.version } };
  }

  async downloadUpdate() {
    this.calls.push("download");
    if (this.downloadFails) throw new Error("connection reset");
    return ["/cache/Aether-1.3.0.AppImage"];
  }

  quitAndInstall(isSilent?: boolean, isForceRunAfter?: boolean) {
    this.calls.push(`install(${String(isSilent)},${String(isForceRunAfter)})`);
  }

  /** Whether anything could install: a direct install, or electron-updater's install on quit. */
  installs(): boolean {
    return this.calls.some((call) => call.startsWith("install")) || this.autoInstallOnAppQuit;
  }
}

function locked(): FakeUpdater {
  const updater = new FakeUpdater();
  lockDown(updater);
  return updater;
}

/** An Ask that answers from `answers` in order and records each prompt it was shown. */
function scripted(answers: boolean[]) {
  const asked: Confirmation[] = [];
  const ask = async (confirmation: Confirmation) => {
    asked.push(confirmation);
    const answer = answers.shift();
    if (answer === undefined) throw new Error("asked more often than scripted");
    return answer;
  };
  return { asked, ask };
}

test("lockDown turns every automatic step off, and the flow refuses an updater that is not", () => {
  const updater = locked();
  assert.deepEqual([updater.autoDownload, updater.autoInstallOnAppQuit, updater.allowDowngrade, updater.allowPrerelease],
    [false, false, false, false]);
  assert.throws(() => new UpdateFlow(new FakeUpdater(), UNSIGNED, scripted([]).ask), /must not download on its own/);
});

test("an update installs only when both the download and the install are confirmed", async () => {
  for (const [download, install] of [[false, false], [false, true], [true, false], [true, true]] as const) {
    const updater = locked();
    const { ask, asked } = scripted(download ? [download, install] : [download]);
    const outcome = await new UpdateFlow(updater, UNSIGNED, ask).run();
    const name = `download ${download}, install ${install}`;
    assert.equal(updater.installs(), download && install, name);
    assert.equal(updater.calls.includes("download"), download, name);
    assert.equal(outcome, !download ? "download-declined" : install ? "installing" : "install-declined", name);
    assert.equal(asked.length, download ? 2 : 1, name);
  }
});

test("the confirmed install is the last call, after both prompts, and never silent", async () => {
  const updater = locked();
  const order: string[] = [];
  const flow = new UpdateFlow(updater, UNSIGNED, async (confirmation) => {
    order.push(`ask ${confirmation.confirm} after [${updater.calls.join(" ")}]`);
    return true;
  });
  assert.equal(await flow.run(), "installing");
  assert.deepEqual(order, ["ask Download after [check]", "ask Install and restart after [check download]"]);
  assert.deepEqual(updater.calls, ["check", "download", "install(false,true)"]);
});

test("no update, or an inactive updater, asks and installs nothing", async () => {
  const updater = locked();
  updater.available = false;
  assert.equal(await new UpdateFlow(updater, UNSIGNED, scripted([]).ask).run(), "up-to-date");
  const inactive = Object.assign(locked(), { checkForUpdates: async () => null });
  assert.equal(await new UpdateFlow(inactive, UNSIGNED, scripted([]).ask).run(), "up-to-date");
  assert.deepEqual(updater.calls, ["check"]);
  assert.equal(updater.installs() || inactive.installs(), false);
});

test("a failed prompt, a failed download or an unlocked updater installs nothing", async () => {
  const broken = locked();
  const failing = async () => { throw new Error("the window is gone"); };
  await assert.rejects(new UpdateFlow(broken, UNSIGNED, failing).run(), /the window is gone/);
  assert.deepEqual(broken.calls, ["check"]);

  const offline = locked();
  offline.downloadFails = true;
  const { ask, asked } = scripted([true]);
  await assert.rejects(new UpdateFlow(offline, UNSIGNED, ask).run(), (error: unknown) =>
    error instanceof ConfirmedStepError && /1\.3\.0 could not be downloaded/.test(error.message));
  assert.equal(asked.length, 1, "no install prompt after a failed download");
  assert.equal(offline.installs(), false);

  const tampered = locked();
  const flow = new UpdateFlow(tampered, UNSIGNED, async () => {
    tampered.autoInstallOnAppQuit = true;
    return true;
  });
  await assert.rejects(flow.run(), /must not install on quit/);
  assert.deepEqual(tampered.calls, ["check"], "nothing downloads once the updater is unlocked");
});

test("a second check while one waits for the user opens no second prompt", async () => {
  const updater = locked();
  let answer: (value: boolean) => void = () => assert.fail("no prompt yet");
  const flow = new UpdateFlow(updater, UNSIGNED, () => new Promise((resolve) => { answer = resolve; }));
  const first = flow.run();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(await flow.run(), "busy");
  answer(false);
  assert.equal(await first, "download-declined");
  assert.deepEqual(updater.calls, ["check"]);
});

test("an unsigned build says so in both prompts before the user confirms", async () => {
  for (const build of [UNSIGNED, SIGNED]) {
    const { ask, asked } = scripted([true, false]);
    await new UpdateFlow(locked(), build, ask).run();
    assert.equal(asked.length, 2);
    for (const prompt of asked) {
      assert.equal(prompt.detail.includes("(unsigned build)"), !build.signed, prompt.detail);
      assert.equal(prompt.detail.includes("This is an unsigned build"), !build.signed, prompt.detail);
      assert.match(prompt.detail, /the GitHub releases of mengsig\/aether/);
      assert.match(prompt.message, /"1\.3\.0"/, "the feed's version fills an escaped slot");
    }
  }
});

test("a hostile version from the feed is refused before any prompt", async () => {
  for (const version of ["1.3.0\nInstall now", "", "9.9.9 (signed)", "1.3", "1.3.0-‮evil", "x".repeat(70)]) {
    const updater = locked();
    updater.version = version;
    const { ask, asked } = scripted([true, true]);
    await assert.rejects(new UpdateFlow(updater, UNSIGNED, ask).run(), /no valid version/, JSON.stringify(version));
    assert.equal(asked.length, 0);
    assert.equal(updater.installs(), false);
  }
});

test("only Linux and signed macOS builds update themselves", () => {
  assert.equal(updatesSupported("linux", UNSIGNED), true);
  assert.equal(updatesSupported("darwin", UNSIGNED), false, "Squirrel.Mac refuses an unsigned app");
  assert.equal(updatesSupported("darwin", SIGNED), true);
  assert.equal(updatesSupported("win32", SIGNED), false);
});

test("a packaged build reads its signing from its package.json, and never claims one it lacks", () => {
  const manifest = (signing: unknown, notarized: unknown) => ({ name: "aether-desktop", aetherBuild: { signing, notarized } });
  assert.deepEqual(buildInfo(manifest("unsigned", false), "1.2.11", true), UNSIGNED);
  assert.deepEqual(buildInfo(manifest("signed", true), "1.2.11", true), SIGNED);
  assert.deepEqual(buildInfo({}, "0.0.0", false), { ...UNSIGNED, version: "0.0.0" }, "a development run is unsigned");
  assert.throws(() => buildInfo({ name: "aether-desktop" }, "1.2.11", true), /records no signing state/);
  assert.throws(() => buildInfo(manifest("maybe", false), "1.2.11", true), /records no signing state/);
  assert.throws(() => buildInfo(manifest("unsigned", true), "1.2.11", true), /notarized but unsigned/);
  assert.throws(() => buildInfo(manifest("signed", "yes"), "1.2.11", true), /no notarization state/);
});
