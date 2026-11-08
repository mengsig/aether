// The Electron side of BridgeHost: dialogs, notifications, links, terminals and the IPC wiring.
// Every handler hands its call to Bridge.handle, which validates before anything runs.

import { spawn } from "node:child_process";
import {
  BrowserWindow, dialog, shell, webContents,
  type IpcMain, type IpcMainInvokeEvent,
} from "electron";
import { requestWithRefusal, type Bridge } from "../bridge/bridge.ts";
import type { Frame } from "../bridge/daemon-link.ts";
import type { Confirmation } from "../security/confirm.ts";
import { BRIDGE_METHODS, type Sender } from "../security/validate.ts";

export const PUSH_CHANNEL = "desktop:push";

export function channel(method: string): string {
  return `desktop:${method}`;
}

/** Who sent `event`: only the main frame of a registered app window passes the validator. */
export function senderOf(event: IpcMainInvokeEvent, appWindows: ReadonlySet<number>): Sender {
  const frame = event.senderFrame;
  return {
    isAppWindow: appWindows.has(event.sender.id),
    isTopFrame: frame !== null && frame.parent === null,
    frameUrl: frame?.url,
  };
}

export function registerIpc(ipcMain: IpcMain, bridge: Bridge, appWindows: ReadonlySet<number>): void {
  for (const method of BRIDGE_METHODS) {
    ipcMain.handle(channel(method), (event, ...args: unknown[]) => {
      const handle = () => bridge.handle(senderOf(event, appWindows), event.sender.id, method, args);
      return method === "request" ? requestWithRefusal(handle, args[1]) : handle();
    });
  }
}

/** Sends a daemon frame to the window that owns its connection; false when that window is gone. */
export function pushFrame(owner: number, conn: string, frame: Frame): boolean {
  const contents = webContents.fromId(owner);
  if (contents === undefined || contents.isDestroyed()) return false;
  contents.send(PUSH_CHANNEL, conn, frame);
  return true;
}

/** The native dialog: Cancel is the default and the escape answer. */
export async function confirmNatively(owner: number, confirmation: Confirmation): Promise<boolean> {
  const contents = webContents.fromId(owner);
  const window = contents === undefined ? null : BrowserWindow.fromWebContents(contents);
  if (window === null) throw new Error("The window that asked for this action is gone");
  const { response } = await dialog.showMessageBox(window, {
    type: "question",
    title: confirmation.title,
    message: confirmation.message,
    detail: confirmation.detail,
    buttons: ["Cancel", confirmation.confirm],
    defaultId: 0,
    cancelId: 0,
    noLink: true,
  });
  return response === 1;
}

/** Runs a fixed sign-in command in a new terminal, detached from the app. */
export function openTerminal(argv: readonly string[]): Promise<void> {
  if (argv.length === 0 || !argv.every((part) => /^[a-z0-9-]+$/.test(part))) throw new Error("a terminal command is plain words");
  const command = process.platform === "darwin"
    ? ["osascript", "-e", `tell application "Terminal" to do script "${argv.join(" ")}"`]
    : ["x-terminal-emulator", "-e", ...argv];
  return new Promise((resolve, reject) => {
    const child = spawn(command[0] as string, command.slice(1), { detached: true, stdio: "ignore" });
    child.once("error", (error) => reject(new Error(`Could not open a terminal for ${argv.join(" ")}`, { cause: error })));
    child.once("spawn", () => {
      child.unref();
      resolve();
    });
  });
}

export function openExternalLink(url: string): Promise<void> {
  return shell.openExternal(url, { activate: true });
}
