// Boots the browser entry: the host, core's app, the registered feature
// packages, the theme and the shell; the service worker in production builds.

import "@aether-ui/aether";
import "@aether-ui/archon";
import "@aether-ui/github";
import "@aether-ui/noesis";
import "@aether-ui/settings";
import { AppProvider, createApp, registerRoutes } from "@aether-ui/core";
import { applyTheme, isThemePreference, resolveTheme, systemPrefersDark, ThemeProvider, type ThemePreference } from "@aether-ui/ui";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./app.css";
import { detectHost } from "./host.ts";
import { acceptPairLink } from "./pair-link.ts";
import { ShellRoot } from "./root.tsx";
import { appRoutes } from "./routes.ts";

const THEME_KEY = "theme";

async function boot(): Promise<void> {
  const mount = document.getElementById("root");
  if (mount === null) throw new Error("index.html has no #root element");
  const host = detectHost({ desktop: (window as { desktop?: unknown }).desktop, localStorage: window.localStorage });
  const stored = await host.storage.get(THEME_KEY);
  const preference: ThemePreference = isThemePreference(stored) ? stored : "system";
  applyTheme(document.documentElement, resolveTheme(preference, systemPrefersDark()));

  // Before the router reads the hash: a pairing URL becomes the pairing route.
  acceptPairLink(window.location, window.history);
  registerRoutes(appRoutes);
  const app = createApp(host);
  const persist = (next: ThemePreference) => {
    host.storage.set(THEME_KEY, next).catch((error: unknown) => app.store.report(`The theme was not saved: ${String(error)}`));
  };
  createRoot(mount).render(
    <StrictMode>
      <AppProvider app={app}>
        <ThemeProvider initialPreference={preference} onPreferenceChange={persist}>
          <ShellRoot />
        </ThemeProvider>
      </AppProvider>
    </StrictMode>,
  );
  if (host.kind === "browser" && import.meta.env.PROD && "serviceWorker" in navigator) {
    navigator.serviceWorker.register("./sw.js", { scope: "./" })
      .catch((error: unknown) => app.store.report(`Offline support is off: the service worker did not register (${String(error)})`));
  }
  app.start().catch((error: unknown) => app.store.report(`Could not reach the daemons: ${error instanceof Error ? error.message : String(error)}`));
}

boot().catch((error: unknown) => {
  const mount = document.getElementById("root");
  if (mount) mount.textContent = `Aether could not start: ${error instanceof Error ? error.message : String(error)}`;
  throw error;
});
