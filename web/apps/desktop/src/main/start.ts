// Starts the desktop app: the app:// scheme, the guarded default session, the bridge and the
// app window. The entry and the headless security harness both start it through here.

import os from "node:os";
import path from "node:path";
import { app, BrowserWindow, ipcMain, protocol, session } from "electron";
import { Bridge, helloFrame, type BridgeHost } from "../bridge/bridge.ts";
import { DaemonLink, type Frame } from "../bridge/daemon-link.ts";
import { findDaemons } from "../discovery/daemons.ts";
import { probe, PROBE_TIMEOUT_MS } from "../discovery/health.ts";
import { machineOf, socketPath, type Machine } from "../discovery/socket-paths.ts";
import { managedStartOn } from "../lifecycle/managed-start.ts";
import { DeepLinkRouter } from "../native/deep-links.ts";
import { notificationSink, pickFolder, windowTarget } from "../native/electron.ts";
import { FolderTokens } from "../native/folder-tokens.ts";
import { Notifier } from "../native/notifications.ts";
import { RemoteWindows } from "../remote/remote-windows.ts";
import { APP_ENTRY, APP_ORIGIN, APP_SCHEME } from "../security/csp.ts";
import { guardSession, pinContents } from "../security/navigation.ts";
import { compileSchemas, readProtocolSchema } from "../security/schema.ts";
import { Validator } from "../security/validate.ts";
import { serveApp } from "./app-protocol.ts";
import { appWindowOptions } from "./app-window.ts";
import { confirmNatively, openExternalLink, openTerminal, pushFrame, registerIpc } from "./host.ts";
import { AETHER_SCHEMA, ARCHON_SCHEMA, PRELOAD, WEB_DIST } from "./paths.ts";
import { PreferenceStore } from "./preferences.ts";

/** Must run before the app is ready: app:// is a standard, secure scheme with fetch. */
export function registerAppScheme(): void {
  protocol.registerSchemesAsPrivileged([
    { scheme: APP_SCHEME, privileges: { standard: true, secure: true, supportFetchAPI: true, codeCache: true } },
  ]);
}

export interface DesktopOptions {
  /** Render offscreen and never show a window: the headless tests. */
  readonly offscreen: boolean;
  readonly env: Readonly<Record<string, string | undefined>>;
  /** Overrides how a privileged action is confirmed; the native dialog otherwise. */
  readonly confirm?: BridgeHost["confirm"];
  /** Where deep links go; the entry makes it before the app is ready, to catch early links. */
  readonly deepLinks?: DeepLinkRouter;
}

export interface Desktop {
  readonly appWindow: BrowserWindow;
  readonly bridge: Bridge;
  readonly remote: RemoteWindows;
  readonly preferences: PreferenceStore;
  readonly appWindowIds: ReadonlySet<number>;
  readonly deepLinks: DeepLinkRouter;
  /** Grants from the folder picker, redeemed by main code only. */
  readonly folders: FolderTokens;
}

interface HostParts {
  readonly remote: RemoteWindows;
  readonly preferences: PreferenceStore;
  readonly protocolVersions: BridgeHost["protocolVersions"];
  readonly deepLinks: DeepLinkRouter;
  readonly folders: FolderTokens;
  push(owner: number, conn: string, frame: Frame): void;
}

/** Discovery, health and managed start over this machine's sockets, the CLIs' own. */
function daemonsOf(machine: Machine, preferences: PreferenceStore, hello: (kind: "aether" | "archon") => Frame) {
  const logDir = app.getPath("logs");
  const managed = managedStartOn(machine, process.cwd(), {
    isEnabled: () => preferences.get("desktop.managed_start") === true,
    isReachable: async (kind) => (await probe(socketPath(kind, machine), { hello: hello(kind), timeoutMs: PROBE_TIMEOUT_MS })).reachable,
    logDir,
  });
  return { find: () => findDaemons(machine, hello), start: (kind: "aether" | "archon") => managed.start(kind), logDir };
}

function host(options: DesktopOptions, parts: HostParts): BridgeHost {
  const { remote, preferences, protocolVersions, push, deepLinks, folders } = parts;
  const version = app.getVersion();
  const daemons = daemonsOf(machineOf(options.env, process.platform, os.userInfo().homedir), preferences,
    (kind) => helloFrame(kind, protocolVersions[kind], version));
  const notifier = new Notifier(notificationSink(), (route) => (route === undefined ? deepLinks.focus() : deepLinks.open(route)));
  return {
    findDaemons: daemons.find,
    openLink: (socket) => DaemonLink.connect(socket),
    push,
    confirm: options.confirm ?? confirmNatively,
    openTerminal,
    notify: (note) => notifier.notify(note),
    startDaemon: daemons.start,
    pickDirectory: (owner, purpose) => pickFolder(owner, purpose, folders),
    openExternal: openExternalLink,
    openRemote: async (origin) => { await remote.open(origin); },
    getPreference: (key) => preferences.get(key),
    setPreference: (key, value) => preferences.set(key, value),
    versions: { app: version, electron: process.versions.electron ?? "", chrome: process.versions.chrome ?? "", node: process.versions.node },
    logPath: daemons.logDir,
    protocolVersions,
  };
}

export async function startDesktop(options: DesktopOptions): Promise<Desktop> {
  if (!app.isReady()) throw new Error("startDesktop runs once the app is ready");
  const schemas = compileSchemas(readProtocolSchema(AETHER_SCHEMA), readProtocolSchema(ARCHON_SCHEMA));
  session.defaultSession.protocol.handle(APP_SCHEME, (request) => serveApp(WEB_DIST, request));
  guardSession(session.defaultSession);
  app.on("web-contents-created", (_event, contents) => {
    contents.on("will-attach-webview", (event) => event.preventDefault());
    contents.setWindowOpenHandler(() => ({ action: "deny" }));
  });
  const extra = options.offscreen ? { offscreen: true } : {};
  const remote = new RemoteWindows(extra);
  const preferences = new PreferenceStore(path.join(app.getPath("userData"), "preferences.json"), (origin) => remote.forget(origin));
  const appWindowIds = new Set<number>();
  const protocolVersions = { aether: schemas.frames.aether.protocolVersion, archon: schemas.frames.archon.protocolVersion };
  const deepLinks = options.deepLinks ?? new DeepLinkRouter();
  const folders = new FolderTokens();
  const bridge: Bridge = new Bridge(new Validator(schemas), host(options, {
    remote, preferences, protocolVersions, deepLinks, folders,
    push: (owner, conn, frame) => { if (!pushFrame(owner, conn, frame)) bridge.connections.closeOwner(owner); },
  }));
  registerIpc(ipcMain, bridge, appWindowIds);
  const base = appWindowOptions(PRELOAD);
  const appWindow = new BrowserWindow({ ...base, webPreferences: { ...base.webPreferences, ...extra } });
  const id = appWindow.webContents.id;
  appWindowIds.add(id);
  pinContents(appWindow.webContents, APP_ORIGIN);
  appWindow.on("closed", () => {
    appWindowIds.delete(id);
    bridge.connections.closeOwner(id);
  });
  await appWindow.loadURL(APP_ENTRY);
  if (!options.offscreen) appWindow.show();
  deepLinks.attach(windowTarget(appWindow));
  return { appWindow, bridge, remote, preferences, appWindowIds, deepLinks, folders };
}
