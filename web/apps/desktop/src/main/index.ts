// The desktop entry: one sandboxed app window over the shared web/dist frontend.

import { app } from "electron";
import { registerAppScheme, startDesktop } from "./start.ts";

app.enableSandbox();
registerAppScheme();
if (!app.requestSingleInstanceLock()) app.quit();

app.whenReady()
  .then(() => startDesktop({ offscreen: false, env: process.env }))
  .then((desktop) => {
    app.on("second-instance", () => desktop.appWindow.focus());
  })
  .catch((error: unknown) => {
    console.error("The desktop app could not start:", error);
    app.exit(1);
  });

app.on("window-all-closed", () => app.quit());
