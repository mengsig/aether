// Packages the desktop app with electron-builder.yml (docs/desktop.md):
//   node build/package.ts [--target linux|mac] [--dir]
// The version is the workspace version; signing follows build/signing.ts; nothing is published.

import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { parseArgs } from "node:util";
import { build, Platform } from "electron-builder";
import { DESKTOP_ROOT, REPO_ROOT } from "../src/main/paths.ts";
import { artifactSuffix, signingConfig, signingFor, type Signing, type Target } from "./signing.ts";

export const OUTPUT_DIR = path.join(REPO_ROOT, "target", "desktop");

/** The `[workspace.package]` version of the repository's Cargo.toml, which release tags carry. */
export function workspaceVersion(cargoToml: string): string {
  const lines = cargoToml.split("\n").map((line) => line.trim());
  const start = lines.indexOf("[workspace.package]");
  if (start === -1) throw new Error("Cargo.toml has no [workspace.package] section");
  const end = lines.findIndex((line, index) => index > start && line.startsWith("["));
  const section = lines.slice(start + 1, end === -1 ? undefined : end);
  const version = section.map((line) => /^version\s*=\s*"([^"]+)"$/.exec(line)?.[1]).find((found) => found !== undefined);
  if (version === undefined || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error("Cargo.toml's [workspace.package] names no release version");
  }
  return version;
}

/** The settings layered over electron-builder.yml for one build. */
export function overrides(target: Target, version: string, signing: Signing): Record<string, unknown> {
  if (version === "") throw new Error("a package has a version");
  return {
    ...signingConfig(target, signing),
    extraMetadata: { version, aetherBuild: { signing: signing.signing, notarized: signing.notarized } },
  };
}

function hostTarget(): Target {
  if (process.platform === "linux") return "linux";
  if (process.platform === "darwin") return "mac";
  throw new Error(`The desktop app is packaged on Linux or macOS, not ${process.platform}`);
}

async function main(): Promise<void> {
  const { values } = parseArgs({ options: { target: { type: "string" }, dir: { type: "boolean", default: false } } });
  const target = (values.target ?? hostTarget()) as Target;
  const signing = signingFor(target, process.env);
  const version = workspaceVersion(readFileSync(path.join(REPO_ROOT, "Cargo.toml"), "utf8"));
  const suffix = artifactSuffix(signing);
  // Only the decision is printed: no credential value reaches the log.
  console.log(`Packaging Aether ${version} for ${target}: ${suffix}${signing.notarized ? "" : ", not notarized"}`);
  process.env.AETHER_DESKTOP_SIGNING = suffix;
  if (signing.signing === "unsigned") process.env.CSC_IDENTITY_AUTO_DISCOVERY = "false";
  const platform = target === "linux" ? Platform.LINUX : Platform.MAC;
  await build({
    projectDir: DESKTOP_ROOT,
    targets: values.dir ? platform.createTarget("dir") : platform.createTarget(),
    config: overrides(target, version, signing),
    publish: "never",
  });
  const made = readdirSync(OUTPUT_DIR).filter((file) => file.includes(`-${suffix}.`) || file.startsWith("latest"));
  if (!values.dir && made.length === 0) throw new Error(`No ${suffix} package was written to ${OUTPUT_DIR}`);
  for (const file of made) console.log(`  ${path.join(OUTPUT_DIR, file)}`);
}

if (path.resolve(process.argv[1] ?? "") === import.meta.filename) {
  main().catch((error: unknown) => {
    console.error("Packaging failed:", error);
    process.exit(1);
  });
}
