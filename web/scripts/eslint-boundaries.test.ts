import path from "node:path";
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

const WEB = path.resolve(import.meta.dirname, "..");
// The workspace's own eslint.config.js, as `npm run lint` uses it.
const eslint = new ESLint({ cwd: WEB });

/** The messages lint gives a source placed at `file` (relative to web/). */
async function lint(file: string, code: string): Promise<string[]> {
  const [result] = await eslint.lintText(code, { filePath: path.join(WEB, file) });
  expect(result).toBeDefined();
  return (result?.messages ?? []).map((message) => `${message.ruleId ?? "parse"}: ${message.message}`);
}

const FEATURE_FILE = "packages/aether/src/fleet/probe.ts";

describe("package import boundaries (CONTRACTS.md §2.2)", () => {
  it("fails a feature package that imports another feature package", async () => {
    const messages = await lint(FEATURE_FILE, 'import { routes } from "@aether-ui/archon";\nexport const all = routes;\n');
    expect(messages).toEqual([
      "aether/import-boundaries: @aether-ui/aether must not import @aether-ui/archon (CONTRACTS.md §2.2); " +
        "feature packages cross over only through routes and the entity-action registry",
    ]);
    const reexport = await lint("packages/settings/src/index.ts", 'export * from "@aether-ui/noesis";\n');
    expect(reexport).toHaveLength(1);
    const relative = await lint(FEATURE_FILE, 'export { routes } from "../../../archon/src/queue/routes.ts";\n');
    expect(relative).toEqual([
      "aether/import-boundaries: a relative import must stay inside packages/aether; import @aether-ui/archon instead",
    ]);
  });

  it("lets a feature package use core, ui, graph, protocol and React", async () => {
    const code = [
      'import { useState } from "react";',
      'import { useCommand } from "@aether-ui/core";',
      'import { Button } from "@aether-ui/ui";',
      'import { GraphCanvas } from "@aether-ui/graph";',
      'import type { RouteDef } from "@aether-ui/protocol";',
      'import { local } from "./local.ts";',
      "export const used: unknown[] = [useState, useCommand, Button, GraphCanvas, local];",
      'export const route: Partial<RouteDef> = { load: () => import("./Screen.tsx") };',
    ].join("\n");
    expect(await lint(FEATURE_FILE, code)).toEqual([]);
  });

  it("keeps protocol free of imports, and ui and graph to React and protocol types", async () => {
    expect(await lint("packages/protocol/src/extra.ts", 'import { useState } from "react";\nexport { useState };\n'))
      .toEqual(["aether/import-boundaries: packages/protocol must not import react (CONTRACTS.md §2.2)"]);
    expect(await lint("packages/ui/src/Shell.tsx", 'import { useStore } from "@aether-ui/core";\nexport { useStore };\n'))
      .toEqual(["aether/import-boundaries: @aether-ui/ui must not import @aether-ui/core (CONTRACTS.md §2.2)"]);
    expect(await lint("packages/ui/src/Shell.tsx", 'import { aether } from "@aether-ui/protocol";\nexport { aether };\n'))
      .toEqual(["aether/import-boundaries: @aether-ui/ui may import only types from @aether-ui/protocol: use `import type`"]);
    expect(await lint("packages/graph/src/force.ts", 'import { forceSimulation } from "d3-force";\nimport type { GNode } from "@aether-ui/protocol";\nexport const f = (n: GNode[]) => forceSimulation(n);\n'))
      .toEqual([]);
    expect(await lint("packages/core/src/layout.ts", 'import { forceSimulation } from "d3-force";\nexport { forceSimulation };\n'))
      .toEqual(["aether/import-boundaries: packages/core must not import d3-force (CONTRACTS.md §2.2)"]);
  });

  it("refuses deep imports, unknown packages and unchecked dynamic imports", async () => {
    expect(await lint(FEATURE_FILE, 'import { store } from "@aether-ui/core/src/store.ts";\nexport { store };\n'))
      .toEqual(["aether/import-boundaries: import @aether-ui/core itself, not a file inside it"]);
    expect(await lint("packages/extras/src/index.ts", "export const x = 1;\n"))
      .toEqual(["aether/import-boundaries: packages/extras is not a package of CONTRACTS.md §2.1"]);
    expect(await lint(FEATURE_FILE, "export const load = (name: string) => import(name);\n"))
      .toEqual(["aether/import-boundaries: a dynamic import needs a string literal, so its package boundary can be checked"]);
  });

  it("checks TypeScript import types and import-require like imports", async () => {
    expect(await lint(FEATURE_FILE, 'export type Other = import("@aether-ui/archon").Other;\n')).toEqual([
      "aether/import-boundaries: @aether-ui/aether must not import @aether-ui/archon (CONTRACTS.md §2.2); " +
        "feature packages cross over only through routes and the entity-action registry",
    ]);
    expect(await lint(FEATURE_FILE, 'export type Queue = import("../../../archon/src/queue/routes.ts").Queue;\n')).toEqual([
      "aether/import-boundaries: a relative import must stay inside packages/aether; import @aether-ui/archon instead",
    ]);
    expect(await lint("packages/ui/src/Shell.tsx", 'export type Def = import("@aether-ui/protocol").RouteDef;\n')).toEqual([]);
    expect(await lint("packages/protocol/src/extra.ts", 'export type El = import("react").ReactNode;\n'))
      .toEqual(["aether/import-boundaries: packages/protocol must not import react (CONTRACTS.md §2.2)"]);
  });

  it("checks the import.meta.glob patterns that load routes", async () => {
    const index = "packages/aether/src/index.ts";
    const local = 'export const routes = import.meta.glob(["./*/routes.ts", "!./**/*.test.ts"], { eager: true });\n';
    expect(await lint(index, local)).toEqual([]);
    const foreign = 'export const routes = import.meta.glob("../../archon/src/**/*.ts", { eager: true });\n';
    expect(await lint(index, foreign)).toEqual([
      "aether/import-boundaries: a relative import must stay inside packages/aether; import @aether-ui/archon instead",
    ]);
    const mixed = 'export const routes = import.meta.glob(["./*/routes.ts", "../../*/src/routes.ts"]);\n';
    expect(await lint(index, mixed)).toEqual(["aether/import-boundaries: a relative import must stay inside packages/aether"]);
    expect(await lint(index, 'export const r = import.meta.glob("/packages/archon/src/*.ts");\n')).toEqual([
      'aether/import-boundaries: import.meta.glob pattern "/packages/archon/src/*.ts" must be relative to this file, with no .. after a wildcard',
    ]);
    for (const climb of ["./*/../../../archon/src/**/routes.ts", "./{..,.}/../archon/src/*.ts"]) {
      expect(await lint(index, `export const r = import.meta.glob(${JSON.stringify(climb)});\n`), climb).toEqual([
        `aether/import-boundaries: import.meta.glob pattern ${JSON.stringify(climb)} must be relative to this file, with no .. after a wildcard`,
      ]);
    }
    const baseRefused = "aether/import-boundaries: import.meta.glob options must be an object literal without `base`, so the matched files stay checkable";
    for (const options of ['{ base: "../../archon/src", eager: true }', '{ "base": "/packages/archon/src" }', "{ ...shared }", "shared"]) {
      const code = `const shared = {};\nexport const r = import.meta.glob("./**/routes.ts", ${options});\nexport { shared };\n`;
      expect(await lint(index, code), options).toEqual([baseRefused]);
    }
    expect(await lint(index, 'export const r = import.meta.glob("./*/routes.ts", { eager: true, import: "routes" });\n')).toEqual([]);
    expect(await lint(index, "const where = './*.ts';\nexport const r = import.meta.glob(where);\n")).toEqual([
      "aether/import-boundaries: import.meta.glob needs string literal patterns, so its package boundary can be checked",
    ]);
  });

  it("checks the files new URL(..., import.meta.url) hands to Vite", async () => {
    const worker = (target: string) => `export const w = new Worker(new URL(${JSON.stringify(target)}, import.meta.url), { type: "module" });\n`;
    expect(await lint("packages/graph/src/layout.ts", worker("./layout.worker.ts"))).toEqual([]);
    expect(await lint("packages/graph/src/layout.ts", worker("../../archon/src/dag/worker.ts"))).toEqual([
      "aether/import-boundaries: a relative import must stay inside packages/graph; import @aether-ui/archon instead",
    ]);
    expect(await lint("packages/graph/src/layout.ts", 'export const site = new URL("../docs", "https://example.com/a/");\n')).toEqual([]);
  });

  it("accepts inline type-only re-exports of protocol from ui, but not runtime ones", async () => {
    const shell = "packages/ui/src/index.ts";
    expect(await lint(shell, 'export { type RouteDef, type RouteProps } from "@aether-ui/protocol";\n')).toEqual([]);
    expect(await lint(shell, 'export { type RouteDef, aether } from "@aether-ui/protocol";\n')).toEqual([
      "aether/import-boundaries: @aether-ui/ui may import only types from @aether-ui/protocol: use `import type`",
    ]);
  });

  it("keeps transports and the desktop bridge inside core", async () => {
    const transport = "Only @aether-ui/core talks to transports (CONTRACTS.md §2.2); send through useCommand.";
    const code = 'export const socket = new WebSocket("ws://127.0.0.1");\n';
    expect(await lint(FEATURE_FILE, code)).toEqual([`no-restricted-globals: Unexpected use of 'WebSocket'. ${transport}`]);
    expect(await lint("packages/core/src/transport.ts", code)).toEqual([]);
    const qualified: Array<[string, string]> = [
      ['export const s = new globalThis.WebSocket("ws://127.0.0.1");\n', "globalThis.WebSocket"],
      ['export const e = new window.EventSource("/events");\n', "window.EventSource"],
      ['export const r = self.fetch("/api");\n', "self.fetch"],
      ['export const s = new globalThis["WebSocket"]("ws://127.0.0.1");\n', "globalThis.WebSocket"],
      ["const { XMLHttpRequest: Xhr } = window;\nexport { Xhr };\n", "window.XMLHttpRequest"],
    ];
    for (const [source, access] of qualified) {
      expect(await lint("packages/settings/src/probe.ts", source), access)
        .toEqual([`no-restricted-properties: '${access}' is restricted from being used. ${transport}`]);
      expect(await lint("packages/core/src/probe.ts", source), access).toEqual([]);
    }
    expect(await lint("packages/ui/src/bridge.ts", "export const bridge = globalThis.desktop;\n")).toEqual([
      "no-restricted-properties: 'globalThis.desktop' is restricted from being used. " +
        "Only @aether-ui/core reaches the desktop bridge (CONTRACTS.md §2.2).",
    ]);
  });
});
