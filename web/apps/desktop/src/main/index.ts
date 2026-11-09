// The desktop entry: one sandboxed app window over the shared web/dist frontend.

import { app } from "electron";
import { DeepLinkRouter } from "../native/deep-links.ts";
import { registerDeepLinkScheme } from "../native/electron.ts";
import { checkForUpdate, labelAboutPanel, readBuildInfo } from "../update/electron.ts";
import { reportSmoke, SMOKE_ENV } from "./smoke.ts";
import { startSingleInstance } from "./single-instance.ts";
import { registerAppScheme, startDesktop, type Desktop } from "./start.ts";

app.enableSandbox();
registerAppScheme();
app.on("window-all-closed", () => app.quit());
startSingleInstance(app, start);

function start(): void {
  // Links can arrive before the window exists: on macOS through open-url, elsewhere in argv.
  const recover = () => {
    void Promise.resolve().then(() => desktopReady).then((desktop) => desktop.ensureMainWindow())
      .then((window) => {
        if (window.isDestroyed() || window.webContents.isCrashed()) throw new Error("The restored app window is unavailable");
        deepLinks.focus();
      }).catch((error: unknown) => console.error("The app window could not be restored:", error));
  };
  const deepLinks = new DeepLinkRouter(recover);
  const build = readBuildInfo();
  const desktopReady: Promise<Desktop> = app.whenReady().then(() => startDesktop({ offscreen: false, env: process.env, deepLinks, build }));
  app.on("open-url", (event, url) => {
    event.preventDefault();
    deepLinks.acceptLink(url);
  });
  app.on("second-instance", (_event, argv) => {
    deepLinks.focus();
    deepLinks.acceptArgv(argv);
  });
  app.on("activate", () => deepLinks.focus());
  deepLinks.acceptArgv(process.argv);
  registerDeepLinkScheme();

  labelAboutPanel(build);

  desktopReady
    .then(async (desktop) => {
      if (process.env[SMOKE_ENV] === "1") await reportSmoke(desktop, build, app);
      else checkForUpdate(desktop.appWindow, build);
    })
    .catch((error: unknown) => {
      console.error("The desktop app could not start:", error);
      app.exit(1);
    });
}
