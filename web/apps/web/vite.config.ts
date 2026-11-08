// The browser entry. `npm run build` writes web/dist/, which the SYNC errand
// commits and aether-remote embeds; gallery.html is a dev-only page.

import path from "node:path";
import { defineConfig } from "vite";
import { packageAliases, WEB_ROOT } from "../../scripts/aliases.ts";
import { SW_FILE, serviceWorkerPlugin } from "./sw-plugin.ts";

const here = import.meta.dirname;

export default defineConfig({
  root: here,
  base: "./",
  resolve: { alias: packageAliases },
  plugins: [serviceWorkerPlugin()],
  build: {
    outDir: path.join(WEB_ROOT, "dist"),
    emptyOutDir: true,
    target: "es2022",
    sourcemap: false,
    rollupOptions: {
      input: { index: path.join(here, "index.html"), "mermaid-frame": path.join(here, "mermaid-frame.html"), sw: path.join(here, "src", "sw.ts") },
      output: {
        entryFileNames: (chunk) => (chunk.name === "sw" ? SW_FILE : "assets/[name]-[hash].js"),
      },
    },
  },
});
