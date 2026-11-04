// The update flow: checking is free, but downloading and installing each wait for the user's own
// confirmation in main. All wording is main's; the feed's version only fills an escaped slot.

import { valueSlot, type Confirmation } from "../security/confirm.ts";

/** The only update source: this repository's GitHub releases (electron-builder.yml `publish`). */
export const UPDATE_FEED = { provider: "github", owner: "mengsig", repo: "aether" } as const;

/** What a build records about its own signing (package.json `aetherBuild`, written when packaged). */
export interface BuildInfo {
  readonly version: string;
  readonly signed: boolean;
  readonly notarized: boolean;
  /** "signed build" or "unsigned build", shown in About, diagnostics and every update prompt. */
  readonly label: string;
}

/** The slice of electron-updater's AppUpdater the flow drives; tests hand in a recording fake. */
export interface Updater {
  autoDownload: boolean;
  autoInstallOnAppQuit: boolean;
  allowDowngrade: boolean;
  allowPrerelease: boolean;
  checkForUpdates(): Promise<{ readonly isUpdateAvailable: boolean; readonly updateInfo: { readonly version: string } } | null>;
  downloadUpdate(): Promise<unknown>;
  quitAndInstall(isSilent?: boolean, isForceRunAfter?: boolean): void;
}

export type Ask = (confirmation: Confirmation) => Promise<boolean>;
export type Outcome = "up-to-date" | "download-declined" | "install-declined" | "installing" | "busy";

/** A failure after the user confirmed a step: the wiring shows it to them, not only in the log. */
export class ConfirmedStepError extends Error {
  override readonly name = "ConfirmedStepError";
}

// A release version as the tags carry it; anything else from the feed is refused before it is shown.
const VERSION = /^\d{1,6}\.\d{1,6}\.\d{1,6}(-[0-9A-Za-z.-]{1,32})?$/;

/** Turns off every automatic step of electron-updater; the flow refuses to run otherwise. */
export function lockDown(updater: Updater): void {
  updater.autoDownload = false;
  updater.autoInstallOnAppQuit = false;
  updater.allowDowngrade = false;
  updater.allowPrerelease = false;
  assertLocked(updater);
}

function assertLocked(updater: Updater): void {
  if (updater.autoDownload !== false) throw new Error("the updater must not download on its own");
  if (updater.autoInstallOnAppQuit !== false) throw new Error("the updater must not install on quit");
  if (updater.allowDowngrade !== false) throw new Error("the updater must not downgrade");
}

function signingNote(build: BuildInfo): string {
  if (!build.signed) {
    return "This is an unsigned build: no publisher signature vouches for it or for its updates. "
      + "The download is checked only against the SHA-512 checksum in the release's metadata.";
  }
  return build.notarized ? "This is a signed and notarized build." : "This is a signed build.";
}

function detail(build: BuildInfo, step: string): string {
  return [
    `You run Aether ${build.version} (${build.label}).`,
    signingNote(build),
    `Source: the GitHub releases of ${UPDATE_FEED.owner}/${UPDATE_FEED.repo}.`,
    step,
  ].join("\n");
}

export function downloadConfirmation(build: BuildInfo, version: string): Confirmation {
  return {
    title: "Update available",
    message: `Aether ${valueSlot(version)} is available. Download it?`,
    detail: detail(build, "Nothing installs until you confirm again once the download is complete."),
    confirm: "Download",
  };
}

export function installConfirmation(build: BuildInfo, version: string): Confirmation {
  return {
    title: "Update ready",
    message: `Install Aether ${valueSlot(version)} and restart?`,
    detail: detail(build, "Aether quits, installs the update and starts again. If you cancel, nothing installs."),
    confirm: "Install and restart",
  };
}

/** One update flow per app; a second run while one is open does nothing. */
export class UpdateFlow {
  private running = false;
  private readonly updater: Updater;
  private readonly build: BuildInfo;
  private readonly ask: Ask;

  constructor(updater: Updater, build: BuildInfo, ask: Ask) {
    assertLocked(updater);
    this.updater = updater;
    this.build = build;
    this.ask = ask;
  }

  async run(): Promise<Outcome> {
    if (this.running) return "busy";
    this.running = true;
    try {
      return await this.steps();
    } finally {
      this.running = false;
    }
  }

  private async steps(): Promise<Outcome> {
    assertLocked(this.updater);
    const found = await this.updater.checkForUpdates();
    if (found === null || !found.isUpdateAvailable) return "up-to-date";
    const version = found.updateInfo.version;
    if (typeof version !== "string" || !VERSION.test(version)) throw new Error("The update feed named no valid version");
    if (!(await this.ask(downloadConfirmation(this.build, version)))) return "download-declined";
    assertLocked(this.updater);
    try {
      await this.updater.downloadUpdate();
    } catch (error) {
      throw new ConfirmedStepError(`The update to ${version} could not be downloaded`, { cause: error });
    }
    if (!(await this.ask(installConfirmation(this.build, version)))) return "install-declined";
    assertLocked(this.updater);
    this.updater.quitAndInstall(false, true);
    return "installing";
  }
}

/** Whether this build can update itself: an unsigned macOS app cannot, as Squirrel.Mac checks signatures. */
export function updatesSupported(platform: NodeJS.Platform, build: BuildInfo): boolean {
  if (platform === "darwin") return build.signed;
  return platform === "linux";
}
