// The desktop entry: one sandboxed app window over the shared web/dist frontend.

import { app } from "electron";
import { DeepLinkRouter } from "../native/deep-links.ts";
import { registerDeepLinkScheme } from "../native/electron.ts";
import { checkForUpdate, labelAboutPanel, readBuildInfo } from "../update/electron.ts";
import { reportSmoke, SMOKE_ENV } from "./smoke.ts";
import { startSingleInstance } from "./single-instance.ts";
import { registerAppScheme, startDesktop } from "./start.ts";

app.enableSandbox();
registerAppScheme();
startSingleInstance(app, start);

function start(): void {
  // Links can arrive before the window exists: on macOS through open-url, elsewhere in argv.
  const deepLinks = new DeepLinkRouter();
  app.on("open-url", (event, url) => {
    event.preventDefault();
    deepLinks.acceptLink(url);
  });
  app.on("second-instance", (_event, argv) => {
    deepLinks.focus();
    deepLinks.acceptArgv(argv);
  });
  deepLinks.acceptArgv(process.argv);
  registerDeepLinkScheme();

  const build = readBuildInfo();
  labelAboutPanel(build);

  app.whenReady()
    .then(() => startDesktop({ offscreen: false, env: process.env, deepLinks, build }))
    .then(async (desktop) => {
      if (process.env[SMOKE_ENV] === "1") await reportSmoke(desktop, build, app);
      else checkForUpdate(desktop.appWindow, build);
    })
    .catch((error: unknown) => {
      console.error("The desktop app could not start:", error);
      app.exit(1);
    });

  app.on("window-all-closed", () => app.quit());
}
