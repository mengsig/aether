import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { build } from "vite";
import { describe, expect, test } from "vitest";
import { fillPlaceholder, precacheList } from "../sw-plugin.ts";

describe("service worker plugin", () => {
  test("precaches the shell first, then every built file but the worker and maps", () => {
    expect(precacheList(["sw.js", "assets/b.js", "index.html", "assets/a.css", "assets/b.js.map", "manifest.webmanifest"]))
      .toEqual(["./", "./assets/a.css", "./assets/b.js", "./manifest.webmanifest"]);
    expect(() => precacheList(["assets/a.js"])).toThrow(/no index.html/);
  });

  test("fills every quoted placeholder and fails when one is missing", () => {
    expect(fillPlaceholder(`a="__X__",b='__X__'`, "__X__", "[1]")).toBe("a=[1],b=[1]");
    expect(() => fillPlaceholder("a=1", "__X__", "[1]")).toThrow(/lost its __X__ placeholder/);
  });

  test("a production build writes sw.js with its list and version, plus the manifest", async () => {
    const outDir = mkdtempSync(path.join(tmpdir(), "aether-web-build-"));
    try {
      await build({ configFile: path.join(import.meta.dirname, "..", "vite.config.ts"), logLevel: "silent", build: { outDir, emptyOutDir: true } });
      const worker = readFileSync(path.join(outDir, "sw.js"), "utf8");
      expect(worker).not.toMatch(/__AETHER_(PRECACHE|VERSION)__/);
      const list = JSON.parse(/\[("\.\/[^\]]*)\]/.exec(worker)?.[0] ?? "[]") as string[];
      expect(list[0]).toBe("./");
      expect(list).toContain("./manifest.webmanifest");
      expect(list.some((file) => /^\.\/assets\/index-[\w-]+\.js$/.test(file))).toBe(true);
      const manifest = JSON.parse(readFileSync(path.join(outDir, "manifest.webmanifest"), "utf8")) as { start_url: string; icons: unknown[] };
      expect(manifest.start_url).toBe("./#/");
      expect(manifest.icons.length).toBeGreaterThan(0);
    } finally {
      rmSync(outDir, { recursive: true, force: true });
    }
  });
});
